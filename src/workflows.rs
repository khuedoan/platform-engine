use crate::workflows::{
    forgejo_bootstrap::ForgejoBootstrapInput, push_to_deploy::PushToDeployInput,
};
use anyhow::{Context, Result, ensure};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use temporalio_client::{
    Client, NamespacedClient, WorkflowStartOptions, errors::WorkflowStartError,
    grpc::WorkflowService, tonic::Code, tonic::IntoRequest,
};
use temporalio_common::{
    data_converters::SerializationContextData,
    protos::{
        coresdk::IntoPayloadsExt,
        temporal::api::{
            common::v1 as common_proto,
            enums::v1::TaskQueueKind,
            schedule::v1 as schedule_proto,
            taskqueue::v1 as taskqueue_proto,
            workflow::v1 as workflow_proto,
            workflowservice::v1::{
                CreateScheduleRequest, DeleteScheduleRequest, UpdateScheduleRequest,
            },
        },
    },
};
use tracing::{error, info, warn};

pub mod forgejo_bootstrap;
pub mod push_to_deploy;

const FORGEJO_BOOTSTRAP_SCHEDULE_ID: &str = "forgejo-bootstrap";
const FORGEJO_BOOTSTRAP_WORKFLOW_ID_PREFIX: &str = "forgejo-bootstrap";
const SCHEDULE_OVERLAP_POLICY_BUFFER_ONE: i32 = 2;
const SCHEDULE_OVERLAP_POLICY_ALLOW_ALL: i32 = 6;

pub async fn start_workflow(client: &Client, id: String, input: PushToDeployInput) -> Result<()> {
    let result = client
        .start_workflow(
            push_to_deploy::PushToDeployWorkflow::run,
            input,
            WorkflowStartOptions::new("main", id).build(),
        )
        .await;

    handle_start_result(result.map(|_| ()))
}

pub async fn start_forgejo_bootstrap(
    client: &Client,
    id: String,
    task_queue: String,
    input: ForgejoBootstrapInput,
) -> Result<()> {
    let result = client
        .start_workflow(
            forgejo_bootstrap::ForgejoBootstrapWorkflow::run,
            input,
            WorkflowStartOptions::new(task_queue, id).build(),
        )
        .await;

    handle_start_result(result.map(|_| ()))
}

pub async fn ensure_forgejo_bootstrap_schedule(
    client: &Client,
    task_queue: String,
    input: ForgejoBootstrapInput,
    interval: Duration,
) -> Result<()> {
    ensure!(
        !interval.is_zero(),
        "FORGEJO_BOOTSTRAP_INTERVAL_SECONDS must be greater than 0"
    );

    let payloads = client
        .options()
        .data_converter
        .to_payloads(&SerializationContextData::Workflow, &input)
        .await
        .context("failed to encode Forgejo bootstrap schedule input")?
        .into_payloads();
    let schedule = forgejo_bootstrap_schedule(task_queue, payloads, interval)?;

    let result = WorkflowService::create_schedule(
        &mut client.clone(),
        CreateScheduleRequest {
            namespace: client.namespace(),
            schedule_id: FORGEJO_BOOTSTRAP_SCHEDULE_ID.to_string(),
            schedule: Some(schedule.clone()),
            initial_patch: Some(schedule_proto::SchedulePatch {
                trigger_immediately: Some(schedule_proto::TriggerImmediatelyRequest {
                    overlap_policy: SCHEDULE_OVERLAP_POLICY_ALLOW_ALL,
                    scheduled_time: None,
                }),
                ..Default::default()
            }),
            identity: client.identity(),
            request_id: request_id("create-forgejo-bootstrap-schedule"),
            ..Default::default()
        }
        .into_request(),
    )
    .await;

    match result {
        Ok(_) => {
            info!("created Forgejo bootstrap Temporal schedule");
            Ok(())
        }
        Err(status) if status.code() == Code::AlreadyExists => {
            WorkflowService::update_schedule(
                &mut client.clone(),
                UpdateScheduleRequest {
                    namespace: client.namespace(),
                    schedule_id: FORGEJO_BOOTSTRAP_SCHEDULE_ID.to_string(),
                    schedule: Some(schedule),
                    identity: client.identity(),
                    request_id: request_id("update-forgejo-bootstrap-schedule"),
                    ..Default::default()
                }
                .into_request(),
            )
            .await
            .context("failed to update Forgejo bootstrap Temporal schedule")?;
            info!("updated Forgejo bootstrap Temporal schedule");
            Ok(())
        }
        Err(status) => Err(status).context("failed to create Forgejo bootstrap Temporal schedule"),
    }
}

pub async fn delete_forgejo_bootstrap_schedule(client: &Client) -> Result<()> {
    let result = WorkflowService::delete_schedule(
        &mut client.clone(),
        DeleteScheduleRequest {
            namespace: client.namespace(),
            schedule_id: FORGEJO_BOOTSTRAP_SCHEDULE_ID.to_string(),
            identity: client.identity(),
        }
        .into_request(),
    )
    .await;

    match result {
        Ok(_) => {
            info!("deleted Forgejo bootstrap Temporal schedule");
            Ok(())
        }
        Err(status) if status.code() == Code::NotFound => Ok(()),
        Err(status) => Err(status).context("failed to delete Forgejo bootstrap Temporal schedule"),
    }
}

fn forgejo_bootstrap_schedule(
    task_queue: String,
    input: Option<common_proto::Payloads>,
    interval: Duration,
) -> Result<schedule_proto::Schedule> {
    Ok(schedule_proto::Schedule {
        spec: Some(schedule_proto::ScheduleSpec {
            interval: vec![schedule_proto::IntervalSpec {
                interval: Some(
                    interval
                        .try_into()
                        .context("invalid Forgejo bootstrap schedule interval")?,
                ),
                phase: None,
            }],
            ..Default::default()
        }),
        action: Some(schedule_proto::ScheduleAction {
            action: Some(schedule_proto::schedule_action::Action::StartWorkflow(
                workflow_proto::NewWorkflowExecutionInfo {
                    workflow_id: FORGEJO_BOOTSTRAP_WORKFLOW_ID_PREFIX.to_string(),
                    workflow_type: Some(common_proto::WorkflowType {
                        name: forgejo_bootstrap::ForgejoBootstrapWorkflow::name().to_string(),
                    }),
                    task_queue: Some(taskqueue_proto::TaskQueue {
                        name: task_queue,
                        kind: TaskQueueKind::Unspecified as i32,
                        normal_name: String::new(),
                    }),
                    input,
                    ..Default::default()
                },
            )),
        }),
        policies: Some(schedule_proto::SchedulePolicies {
            overlap_policy: SCHEDULE_OVERLAP_POLICY_BUFFER_ONE,
            ..Default::default()
        }),
        state: Some(schedule_proto::ScheduleState {
            notes: "Managed by platform-engine".to_string(),
            ..Default::default()
        }),
    })
}

fn request_id(action: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("platform-engine-{action}-{timestamp}")
}

fn handle_start_result(result: std::result::Result<(), WorkflowStartError>) -> Result<()> {
    match result {
        Ok(()) => {
            info!("workflow started");
            Ok(())
        }
        Err(WorkflowStartError::AlreadyStarted { .. }) => {
            warn!("workflow already exists");
            Ok(())
        }
        Err(err) => {
            error!(error = %err, "failed to start workflow");
            Err(err.into())
        }
    }
}
