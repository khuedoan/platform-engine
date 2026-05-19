use std::time::Duration;

use temporalio_sdk::ActivityOptions;

const COMMAND_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn command_activity_options(timeout: Duration) -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(timeout)
        .heartbeat_timeout(COMMAND_HEARTBEAT_TIMEOUT)
        .build()
}
