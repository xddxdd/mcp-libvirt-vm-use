//! `rmcp` glue: the tool router, the async tool handlers and the server handler.
//!
//! Tool parameter structs and the implementation of every tool live in
//! [`crate::tools`]; this module only adapts them to the MCP layer. SPICE
//! operations are awaited directly (they are async); only the libvirt lookup
//! inside them runs on the blocking pool. The server name and version are
//! literals in the `#[tool]`/`#[tool_handler]` attributes below (the macros
//! require literals); the constants here are used for the startup diagnostics
//! on stderr.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};

use crate::libvirt::Libvirt;
use crate::tools::{
    self, KeyPressParams, MouseClickParams, MouseDragParams, MouseMoveParams, MouseScrollParams,
    ScreenshotParams, TypeTextParams,
};

pub const SERVER_NAME: &str = "mcp-libvirt-vm-use";
pub const SERVER_VERSION: &str = "0.1.0";

#[derive(Clone)]
pub struct LibvirtTools {
    tool_router: ToolRouter<LibvirtTools>,
    libvirt: Arc<Libvirt>,
}

#[tool_router]
impl LibvirtTools {
    pub fn new(libvirt: Arc<Libvirt>) -> Self {
        LibvirtTools {
            tool_router: LibvirtTools::tool_router(),
            libvirt,
        }
    }

    #[tool(description = "List all libvirt domains with their id, state and SPICE display endpoint.")]
    async fn list_domains(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(blocking_libvirt(move || tools::list_domains(&libvirt)).await)
    }

    #[tool(description = "Capture the SPICE display of a domain and return it as a PNG image.")]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::screenshot(&libvirt, params).await))
    }

    #[tool(description = "Type ASCII text into a domain through the SPICE inputs channel.")]
    async fn type_text(
        &self,
        Parameters(params): Parameters<TypeTextParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::type_text(&libvirt, params).await))
    }

    #[tool(
        description = "Send a key combination such as \"ctrl+alt+t\" or a single key such as \"enter\" to a domain."
    )]
    async fn key_press(
        &self,
        Parameters(params): Parameters<KeyPressParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::key_press(&libvirt, params).await))
    }

    #[tool(description = "Move the mouse pointer to an absolute position inside a domain's display.")]
    async fn mouse_move(
        &self,
        Parameters(params): Parameters<MouseMoveParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::mouse_move(&libvirt, params).await))
    }

    #[tool(description = "Click a mouse button in a domain, optionally moving to a position first.")]
    async fn mouse_click(
        &self,
        Parameters(params): Parameters<MouseClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::mouse_click(&libvirt, params).await))
    }

    #[tool(description = "Scroll the mouse wheel inside a domain's display.")]
    async fn mouse_scroll(
        &self,
        Parameters(params): Parameters<MouseScrollParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::mouse_scroll(&libvirt, params).await))
    }

    #[tool(
        description = "Press the left mouse button at one position, drag to another position and release."
    )]
    async fn mouse_drag(
        &self,
        Parameters(params): Parameters<MouseDragParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let libvirt = Arc::clone(&self.libvirt);
        Ok(tool_result(tools::mouse_drag(&libvirt, params).await))
    }
}

#[tool_handler(router = self.tool_router, name = "mcp-libvirt-vm-use", version = "0.1.0")]
impl ServerHandler for LibvirtTools {}

/// Turn a tool implementation result into an MCP result: operational failures
/// are reported as `isError` content so the caller sees the message.
fn tool_result(result: Result<Vec<ContentBlock>, String>) -> CallToolResult {
    match result {
        Ok(content) => CallToolResult::success(content),
        Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
    }
}

/// Run a synchronous libvirt-only tool implementation off the async runtime.
async fn blocking_libvirt<F>(operation: F) -> CallToolResult
where
    F: FnOnce() -> Result<Vec<ContentBlock>, String> + Send + 'static,
{
    match tokio::task::spawn_blocking(operation).await {
        Ok(result) => tool_result(result),
        Err(join_error) => CallToolResult::error(vec![ContentBlock::text(format!(
            "internal error while running the tool: {}",
            join_error
        ))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{MouseButton, ScrollDirection};

    fn server() -> LibvirtTools {
        LibvirtTools::new(Arc::new(Libvirt {
            uri: "qemu:///system".to_string(),
        }))
    }

    fn tool_result_text(result: &CallToolResult) -> String {
        match result.content.first() {
            Some(ContentBlock::Text(text)) => text.text.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn tool_router_exposes_the_eight_tools() {
        let tools = LibvirtTools::tool_router().list_all();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        // `list_all` returns the tools sorted by name.
        assert_eq!(
            names,
            vec![
                "key_press",
                "list_domains",
                "mouse_click",
                "mouse_drag",
                "mouse_move",
                "mouse_scroll",
                "screenshot",
                "type_text"
            ]
        );
        for tool in &tools {
            let description = tool.description.as_ref().expect("tool description");
            assert!(!description.is_empty(), "{} has no description", tool.name);
            assert_eq!(
                tool.input_schema.get("type").and_then(|value| value.as_str()),
                Some("object"),
                "{} has no object input schema",
                tool.name
            );
        }
    }

    #[test]
    fn schema_documents_required_and_optional_parameters() {
        let tools = LibvirtTools::tool_router().list_all();
        let screenshot = tools
            .iter()
            .find(|tool| tool.name == "screenshot")
            .expect("screenshot tool");
        let properties = &screenshot.input_schema["properties"];
        assert_eq!(properties["domain"]["type"], "string");
        // Optional fields are nullable integers in the generated schema.
        assert_eq!(properties["wait_ms"]["type"], serde_json::json!(["integer", "null"]));
        assert_eq!(screenshot.input_schema["required"], serde_json::json!(["domain"]));
        // Field doc comments are the property descriptions.
        let description = properties["wait_ms"]["description"]
            .as_str()
            .expect("wait_ms description");
        assert!(description.contains("default 500"), "got: {}", description);

        let list_domains = tools
            .iter()
            .find(|tool| tool.name == "list_domains")
            .expect("list_domains tool");
        assert_eq!(list_domains.input_schema["properties"], serde_json::json!({}));
        assert_eq!(list_domains.input_schema["type"], "object");
    }

    #[test]
    fn schema_advertises_the_button_and_direction_enums() {
        let tools = LibvirtTools::tool_router().list_all();
        let mouse_click = tools
            .iter()
            .find(|tool| tool.name == "mouse_click")
            .expect("mouse_click tool");
        assert_eq!(
            referenced_definition(&mouse_click.input_schema, "button")["enum"],
            serde_json::json!(["left", "middle", "right"])
        );
        assert_eq!(
            mouse_click.input_schema["required"],
            serde_json::json!(["domain", "button"])
        );

        let mouse_scroll = tools
            .iter()
            .find(|tool| tool.name == "mouse_scroll")
            .expect("mouse_scroll tool");
        assert_eq!(
            referenced_definition(&mouse_scroll.input_schema, "direction")["enum"],
            serde_json::json!(["up", "down"])
        );
    }

    /// Follow the `$ref` of a property to the definition it points at.
    fn referenced_definition<'a>(
        schema: &'a serde_json::Map<String, serde_json::Value>,
        property: &str,
    ) -> &'a serde_json::Value {
        let reference = schema["properties"][property]["$ref"]
            .as_str()
            .expect("property must reference a definition");
        let name = reference.rsplit('/').next().expect("definition name");
        &schema["$defs"][name]
    }

    #[test]
    fn router_only_routes_declared_tools() {
        let router = LibvirtTools::tool_router();
        assert!(router.get("screenshot").is_some());
        assert!(router.get("teleport").is_none());
    }

    #[test]
    fn server_name_and_version_are_reported() {
        let info = ServerHandler::get_info(&server());
        assert_eq!(info.server_info.name, SERVER_NAME);
        assert_eq!(info.server_info.version, SERVER_VERSION);
        assert!(info.capabilities.tools.is_some(), "tools capability missing");
    }

    #[tokio::test]
    async fn operation_errors_are_tool_errors_with_text() {
        let result = server()
            .mouse_click(Parameters(MouseClickParams {
                domain: "Windows10".to_string(),
                button: MouseButton::Left,
                x: Some(3),
                y: None,
                double_click: None,
            }))
            .await
            .expect("tool-level failures are results, not protocol errors");
        assert_eq!(result.is_error, Some(true));
        let text = tool_result_text(&result);
        assert!(text.contains("'x' and 'y'"), "got: {}", text);
    }

    #[tokio::test]
    async fn libvirt_failures_are_readable_tool_errors() {
        let result = server()
            .mouse_scroll(Parameters(MouseScrollParams {
                domain: "unreachable-domain".to_string(),
                direction: ScrollDirection::Down,
                clicks: Some(2),
            }))
            .await
            .expect("tool-level failures are results, not protocol errors");
        // Whatever libvirt says, the caller gets a readable error, never a
        // panic and never a bare protocol error.
        assert_eq!(result.is_error, Some(true));
        let text = tool_result_text(&result);
        assert!(!text.is_empty(), "the error must reach the caller as text");
    }
}
