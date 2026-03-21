use anyhow::Result;
use mxdx_matrix::MatrixClient;
use tracing::info;

pub struct CoordinatorBot {
    #[allow(dead_code)]
    matrix_client: MatrixClient,
}

impl CoordinatorBot {
    pub fn new(matrix_client: MatrixClient) -> Self {
        Self { matrix_client }
    }

    pub async fn run(&self) -> Result<()> {
        info!("coordinator running");
        Ok(())
    }
}
