use goat_tool::ToolInvocation;
use goat_tool_search::{NativeSearchFuture, NativeSearchRequest, NativeSearchService};

use crate::LoopEnv;

pub(crate) struct EngineNativeSearchService;

impl NativeSearchService for EngineNativeSearchService {
    fn search<'a>(
        &'a self,
        request: NativeSearchRequest,
        invocation: ToolInvocation<'a>,
    ) -> NativeSearchFuture<'a> {
        Box::pin(async move {
            let env = invocation
                .host
                .and_then(|host| host.downcast_ref::<LoopEnv>())
                .ok_or_else(|| "native search environment unavailable".to_owned())?;
            if !env.provider.supports_web_search() {
                return Err("native web search is not supported".to_owned());
            }
            let handle = env.provider.web_search(request.query);
            let abort = handle.abort_handle();
            let output = tokio::select! {
                biased;
                () = invocation.cancellation.cancelled() => {
                    abort.abort();
                    return Err("interrupted".to_owned());
                }
                joined = handle => joined
                    .map_err(|err| format!("web search task failed: {err}"))?
                    .map_err(|err| err.to_string())?,
            };
            Ok(output.content)
        })
    }
}
