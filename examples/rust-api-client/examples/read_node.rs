use std::{env, error::Error, fs, time::Duration};

use xelab_api_client_example::XeLabClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let origin = env::var("XELAB_API_ORIGIN")?;
    let bearer = env::var("XELAB_API_TOKEN")?;
    let ca = fs::read(env::var("XELAB_API_CA_FILE")?)?;
    let client = XeLabClient::new(&origin, &bearer, &ca, Duration::from_secs(10))?;
    let node = client.get_node().await?;
    println!(
        "revision={} admission_enabled={}",
        node.revision, node.admission_enabled
    );
    Ok(())
}
