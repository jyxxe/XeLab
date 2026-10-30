pub mod admission;
pub mod api;
pub mod artifact;
pub mod auth;
pub mod broker;
pub mod config;
pub mod contracts;
pub mod controller;
pub mod db;
pub mod error;
pub mod runtime;
pub mod tls;

pub mod rpc {
    tonic::include_proto!("xelab.v1");
}

pub const SPEC_VERSION: &str = "2.8.0";
