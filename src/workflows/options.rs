use std::time::Duration;

use temporalio_common::protos::temporal::api::common::v1::RetryPolicy;
use temporalio_sdk::ActivityOptions;

const COMMAND_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_INITIAL_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const COMMAND_MAXIMUM_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const COMMAND_MAXIMUM_ATTEMPTS: i32 = 3;

pub(crate) fn command_activity_options(timeout: Duration) -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(timeout)
        .heartbeat_timeout(COMMAND_HEARTBEAT_TIMEOUT)
        .retry_policy(RetryPolicy {
            initial_interval: Some(
                COMMAND_INITIAL_RETRY_INTERVAL
                    .try_into()
                    .expect("valid retry interval"),
            ),
            backoff_coefficient: 2.0,
            maximum_interval: Some(
                COMMAND_MAXIMUM_RETRY_INTERVAL
                    .try_into()
                    .expect("valid retry interval"),
            ),
            maximum_attempts: COMMAND_MAXIMUM_ATTEMPTS,
            non_retryable_error_types: Vec::new(),
        })
        .build()
}
