/*
 * Pets API
 *
 * No description provided (generated by Openapi Generator https://github.com/openapitools/openapi-generator)
 *
 * The version of the OpenAPI document: 1.0.0
 * 
 * Generated by: https://openapi-generator.tech
 */

use crate::models;

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dog {
    #[serde(rename = "uuid", skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "breed")]
    pub breed: String,
    #[serde(rename = "age")]
    pub age: i32,
}

impl Dog {
    pub fn new(name: String, breed: String, age: i32) -> Dog {
        Dog {
            uuid: None,
            name,
            breed,
            age,
        }
    }
}

