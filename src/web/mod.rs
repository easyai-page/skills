pub mod api;

use crate::core::{error::Result, paths::Layout};

pub fn run(layout: &Layout, port: u16, no_open: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().map_err(crate::core::error::Error::Io)?;
    rt.block_on(async {
        let app = api::router(api::AppState::new(layout.clone()));
        let addr = format!("127.0.0.1:{port}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        println!("Web UI: http://{addr}");
        if !no_open {
            let _ = open::that(format!("http://{addr}"));
        }
        axum::serve(listener, app).await?;
        Ok(())
    })
}
