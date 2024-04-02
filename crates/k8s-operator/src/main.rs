use anyhow::{Context, Result};
use core::fmt::Debug;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, PostParams, WatchEvent, WatchParams};
use kube::Client;
use kube::CustomResourceExt;
use kube::ResourceExt;
use kube_runtime::conditions;
use kube_runtime::wait::await_condition;
use log::{debug, error, info, warn};
use openapi::apis::configuration::Configuration;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};
use warp::Filter;

async fn watch_resource<T, F>(
    config: Arc<Configuration>,
    api: Api<T>,
    handle: F,
) -> anyhow::Result<()>
where
    T: ResourceExt + Clone + Debug + Send + DeserializeOwned + 'static,
    F: Fn(Arc<Configuration>, WatchEvent<T>, Api<T>) -> anyhow::Result<()> + Send + Sync + 'static,
{
    loop {
        let watcher_stream = api.watch(&WatchParams::default(), "0").await?;
        let events = watcher_stream.boxed();
        process_events(events, config.clone(), api.clone(), &handle).await;
        sleep(Duration::from_secs(5)).await;
    }
}

async fn process_events<'a, T, F>(
    events: BoxStream<'a, Result<WatchEvent<T>, kube::Error>>,
    config: Arc<Configuration>,
    api: Api<T>,
    handle: &'a F,
) where
    T: ResourceExt + Clone + Debug + Send + DeserializeOwned + 'static,
    F: Fn(Arc<Configuration>, WatchEvent<T>, Api<T>) -> anyhow::Result<()> + Send + Sync + 'static,
{
    events
        .for_each(|event| {
            let config = config.clone();
            let api = api.clone();
            let handle = handle.clone();
            async move {
                match event {
                    Ok(event) => {
                        if let Err(e) = handle(config, event, api) {
                            error!("Error handling event: {:?}", e);
                        }
                    }
                    Err(e) => {
                        error!("Error watching events: {:?}", e);
                    }
                }
            }
        })
        .await;
}

async fn deploy_crd(
    kube_client: Api<CustomResourceDefinition>,
    crd: CustomResourceDefinition,
) -> Result<()> {
    let crd_name = crd
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| String::from("Unnamed CRD"));
    info!("Deploying CRD: {}", crd_name);

    let result = kube_client.create(&PostParams::default(), &crd).await;

    match result {
        core::result::Result::Ok(_) => info!("Successfully created CRD: {}", crd_name),
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            if kube_client
                .replace(&crd_name, &PostParams::default(), &crd)
                .await
                .is_ok()
            {
                info!("Successfully updated CRD: {}", crd_name);
            } else {
                warn!("Failed to update CRD, already exists: {}", crd_name);
            }
        }
        Err(_) => error!("Failed to create CRD: {}", crd_name),
    }

    Ok(())
}

async fn wait_for_crd(kube_client: Api<CustomResourceDefinition>, crd_name: &str) -> Result<()> {
    info!(
        "Waiting for the api-server to accept the CRD of {}...",
        crd_name
    );

    let establish = await_condition(
        kube_client.clone(),
        crd_name,
        conditions::is_crd_established(),
    );
    let _ = timeout(std::time::Duration::from_secs(10), establish).await?;
    info!("CRD of {} is established.", crd_name);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    info!("Starting operator...");

    let access_token = std::env::var("ACCESS_TOKEN").context("ACCESS_TOKEN is not set")?;
    let client = Client::try_default().await?;
    let config = Arc::new(Configuration {
        base_path: "http://localhost:8080".to_string(),
        client: reqwest::Client::new(),
        user_agent: Some("k8s-operator".to_string()),
        bearer_access_token: Some(access_token),
        ..Default::default()
    });

    let kube_client: Api<CustomResourceDefinition> = Api::all(client);

    if std::env::var("INSTALL_CRDS")
        .unwrap_or_default()
        .to_lowercase()
        == "true"
    {
        info!("INSTALL_CRDS is set to true. Deploying CRDs...");

        let crds = vec![
            k8s_operator::types::cat::Cat::crd(),
            k8s_operator::types::dog::Dog::crd(),
            k8s_operator::types::horse::Horse::crd(),
        ];

        for crd in crds {
            deploy_crd(kube_client.clone(), crd).await?;
        }
    }

    let controllers = vec![
        format!("cats.example.com"),
        format!("dogs.example.com"),
        format!("horses.example.com"),
    ];
    for controller in controllers {
        if let Err(e) = wait_for_crd(kube_client.clone(), &controller).await {
            error!("Error waiting for CRD {}: {}", &controller, e);
        }
    }

    // add controllers for cats.example.com/v1 here

    // add controllers for dogs.example.com/v1 here

    // add controllers for horses.example.com/v1 here

    tokio::spawn(async {
        let liveness_route = warp::path!("healthz")
            .map(|| warp::reply::with_status("OK", warp::http::StatusCode::OK));

        let readiness_route = warp::path!("readyz")
            .map(|| warp::reply::with_status("OK", warp::http::StatusCode::OK));

        let health_routes = liveness_route.or(readiness_route);

        warp::serve(health_routes).run(([0, 0, 0, 0], 8000)).await;
    });

    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl+C")?;
    println!("Termination signal received. Shutting down.");

    Ok(())
}