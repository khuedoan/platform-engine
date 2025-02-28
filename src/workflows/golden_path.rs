use temporal_sdk::{WfContext, WfExitValue, WorkflowResult};
use tracing::info;

pub fn name() -> String {
    "golden_path".to_string()
}

pub async fn run(_ctx: WfContext) -> WorkflowResult<String> {
    info!("running workflow");
    info!("workflow completed");

    Ok(WfExitValue::Normal("done".to_string()))
}
