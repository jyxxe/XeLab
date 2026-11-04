//! Request authority used by existing API component fixtures. In-process
//! requests must carry the same explicit Host as real HTTP clients.
pub fn authority(configured_origin: Option<&str>) -> String {
    let origin = if let Some(origin) = configured_origin {
        url::Url::parse(origin).unwrap()
    } else {
        static EXAMPLE: std::sync::OnceLock<url::Url> = std::sync::OnceLock::new();
        EXAMPLE
            .get_or_init(|| {
                let config: toml::Value = toml::from_str(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../docs/contracts/examples/kernel.toml"
                )))
                .unwrap();
                url::Url::parse(config["server"]["public_origin"].as_str().unwrap()).unwrap()
            })
            .clone()
    };
    origin[url::Position::BeforeHost..url::Position::AfterPort].to_owned()
}
