use mxdx_matrix::MatrixClient;

pub struct SenderClient {
    #[allow(dead_code)]
    matrix_client: MatrixClient,
}

impl SenderClient {
    pub fn new(matrix_client: MatrixClient) -> Self {
        Self { matrix_client }
    }
}
