pub(crate) struct ToolLineCtx<'a> {
    pub cwd: &'a str,
    pub width: u16,
    pub failed: bool,
}

pub(crate) fn transcript_sig(
    tool_name: &str,
    display_primary: &str,
    ctx: &ToolLineCtx<'_>,
) -> String {
    goat_tool::gist::transcript_sig(tool_name, display_primary, ctx.cwd, ctx.width, ctx.failed)
}
