use mxdx_matrix::MatrixClient;

pub struct WorkerClient {
    #[allow(dead_code)]
    matrix_client: MatrixClient,
    #[allow(dead_code)]
    worker_id: String,
}

impl WorkerClient {
    pub fn new(matrix_client: MatrixClient, worker_id: String) -> Self {
        Self {
            matrix_client,
            worker_id,
        }
    }
}
