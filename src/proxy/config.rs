#[allow(dead_code)]
pub struct ProxyConfig {
    pub timeout: u64,
    pub container: String,
    pub cmd: Vec<String>
}
