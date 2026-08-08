pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub parameters: serde_json::Value,
}
