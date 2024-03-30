use futures_util::stream::StreamExt;
use k8s_openapi::api::core::v1::{Event, EventSource, ObjectReference};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use k8s_openapi::chrono;
use kube::api::{
    Api, ListParams, ObjectMeta, Patch, PatchParams, PostParams, WatchEvent, WatchParams,
};
use kube::core::CustomResourceExt;
use kube::{Client, Resource, ResourceExt};
use log::{debug, error, info, warn};
use openapi::apis::configuration::Configuration;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub mod types;

pub mod controllers;

pub async fn watch_resource<T>(
    config: Arc<Configuration>,
    kubernetes_api: Api<T>,
    watch_params: WatchParams,
    handler: fn(Arc<Configuration>, WatchEvent<T>, Api<T>),
) -> anyhow::Result<()>
where
    T: Clone + core::fmt::Debug + DeserializeOwned + 'static,
{
    let mut stream = kubernetes_api.watch(&watch_params, "0").await?.boxed();
    loop {
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => handler(Arc::clone(&config), event, kubernetes_api.clone()),
                Err(e) => error!("Error watching resource: {:?}", e),
            }
        }
        sleep(Duration::from_secs(1)).await;
        stream = kubernetes_api.watch(&watch_params, "0").await?.boxed();
    }
}

pub async fn add_finalizer<T>(resource: &mut T, kubernetes_api: Api<T>)
where
    T: Clone
        + Serialize
        + DeserializeOwned
        + Resource
        + CustomResourceExt
        + core::fmt::Debug
        + 'static,
{
    let finalizer = String::from(format!("finalizers.{}", "example.com"));
    let finalizers = resource.meta_mut().finalizers.get_or_insert_with(Vec::new);
    if finalizers.contains(&finalizer) {
        debug!("Finalizer already exists");
        return;
    }
    finalizers.push(finalizer);
    let resource_name = resource.meta_mut().name.clone().unwrap();
    let resource_clone = resource.clone();
    let patch = Patch::Merge(&resource_clone);
    let patch_params = PatchParams {
        field_manager: resource.meta_mut().name.clone(),
        ..Default::default()
    };
    match kubernetes_api
        .patch(&resource_name, &patch_params, &patch)
        .await
    {
        Ok(_) => debug!("Finalizer added successfully"),
        Err(e) => debug!("Failed to add finalizer: {:?}", e),
    };
}

pub async fn remove_finalizer<T>(resource: &mut T, kubernetes_api: Api<T>)
where
    T: Clone
        + Serialize
        + DeserializeOwned
        + Resource
        + CustomResourceExt
        + core::fmt::Debug
        + 'static,
{
    let finalizer = String::from(format!("finalizers.{}", "example.com"));
    if let Some(finalizers) = &mut resource.meta_mut().finalizers {
        if finalizers.contains(&finalizer) {
            finalizers.retain(|f| f != &finalizer);
            let patch = json ! ({ "metadata" : { "finalizers" : finalizers } });
            let patch = Patch::Merge(&patch);
            let patch_params = PatchParams {
                field_manager: resource.meta_mut().name.clone(),
                ..Default::default()
            };
            match kubernetes_api
                .patch(
                    &resource.clone().meta_mut().name.clone().unwrap(),
                    &patch_params,
                    &patch,
                )
                .await
            {
                Ok(_) => debug!("Finalizer removed successfully"),
                Err(e) => debug!("Failed to remove finalizer: {:?}", e),
            };
        }
    }
}

pub async fn add_event<T>(kind: String, resource: &mut T, reason: &str, from: &str, message: &str)
where
    T: CustomResourceExt
        + Clone
        + Serialize
        + DeserializeOwned
        + Resource
        + core::fmt::Debug
        + 'static,
{
    let kube_client = kube::Client::try_default().await.unwrap();
    let namespace = resource.namespace().clone().unwrap_or_default();
    let kubernetes_api: Api<Event> = Api::namespaced(kube_client.clone(), &namespace);
    let resource_ref = ObjectReference {
        kind: Some(kind),
        namespace: resource.namespace().clone(),
        name: Some(resource.meta().name.clone().unwrap()),
        uid: resource.uid().clone(),
        ..Default::default()
    };
    let timestamp = chrono::Utc::now().to_rfc3339();
    let event_name = format!("{}-{}", resource.meta().name.clone().unwrap(), timestamp);
    let new_event = Event {
        metadata: ObjectMeta {
            name: Some(event_name),
            ..Default::default()
        },
        involved_object: resource_ref,
        reason: Some(reason.into()),
        message: Some(message.into()),
        type_: Some("Normal".into()),
        source: Some(EventSource {
            component: Some(String::from(from)),
            ..Default::default()
        }),
        first_timestamp: Some(Time(chrono::Utc::now())),
        last_timestamp: Some(Time(chrono::Utc::now())),
        ..Default::default()
    };
    match kubernetes_api
        .create(&PostParams::default(), &new_event)
        .await
    {
        Ok(_) => debug!("Event added successfully"),
        Err(e) => debug!("Failed to add event: {:?}", e),
    };
}

pub async fn create_condition(
    status: &str,
    type_: &str,
    reason: &str,
    message: &str,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        last_transition_time: Time(chrono::Utc::now()),
        message: message.to_string(),
        reason: reason.to_string(),
        status: status.to_string(),
        type_: type_.to_string(),
        observed_generation: observed_generation,
    }
}

pub async fn update_status<T>(kubernetes_api: Api<T>, status: T)
where
    T: kube::Resource<DynamicType = ()>
        + serde::Serialize
        + Clone
        + for<'de> serde::Deserialize<'de>,
{
    let resource_name = status.meta().name.clone().unwrap_or_else(|| String::new());

    if resource_name.is_empty() {
        warn!("Resource name is empty, cannot update status");
        return;
    }

    let status_replace_bytes = match serde_json::to_vec(&status) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to serialize status: {:?}", e);
            return;
        }
    };

    let post_params = PostParams::default();
    // let mut retries = 0;
    // while retries < 5 {
    match kubernetes_api
        .replace_status(&resource_name, &post_params, status_replace_bytes.clone())
        .await
    {
        Ok(_) => {
            println!("Status updated successfully for {}", resource_name);
            // break;
        }
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            println!(
                "Conflict updating status for {}, retrying...",
                resource_name
            );
            // retries += 1;
            sleep(Duration::from_secs(1)).await;
            // continue;
        }
        Err(e) => {
            println!("Failed to update status for {}: {:?}", resource_name, e);
            // break;
        }
    }
    // }
}

pub async fn crds_exist(client: Client, group: &str) -> anyhow::Result<bool> {
    let crds: Api<CustomResourceDefinition> = Api::all(client);
    let lp = ListParams::default();
    let crd_list = crds.list(&lp).await?;

    Ok(crd_list.items.iter().any(|crd| crd.spec.group == group))
}
