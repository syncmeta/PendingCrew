use super::admin_execution::Socket;
use super::*;
use crewd_proto::{TodoCreateParams, WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};

#[tokio::test]
async fn exact_captain_ruling_survives_reopen_without_replay() {
    let mut f = Fixture::new().await;
    let crew = f.crew.clone();
    let (id, captain) = f
        .state
        .collab(move |tx| {
            let id = requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: crew.clone(),
                    text: "synthetic captain ruling requirement".into(),
                    channel_id: None,
                },
            )?
            .todo
            .requirement_id
            .unwrap();
            let plan = crewd_collab::work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.clone(),
                    title: "not viable".into(),
                    sources: vec![id.clone()],
                    ..Default::default()
                },
            )?;
            crewd_collab::work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: plan.id,
                    expected_revision: plan.revision,
                    status: Some(WorkUpdateStatus::Dropped),
                    note: Some("verified dropped rationale".into()),
                    ..Default::default()
                },
            )?;
            Ok((id, crews::captain_of(tx.conn(), &crew)?.unwrap()))
        })
        .await
        .unwrap();
    let tool = |name, args| json!({"tool":name,"args":args});
    let mut worker = Socket::attach(f.state.clone(), &f.worker).await;
    let denied = worker
        .call("agent.call", tool("todo_ruling_check", json!({"id":id})))
        .await
        .unwrap();
    assert_eq!(denied["isError"], true);
    worker.close().await;

    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    let checked = captain_socket
        .call("agent.call", tool("todo_ruling_check", json!({"id":id})))
        .await
        .unwrap();
    assert_eq!(checked["isError"], false, "{checked}");
    let check: crewd_proto::RequirementRulingCheck =
        serde_json::from_str(checked["text"].as_str().unwrap()).unwrap();
    assert_eq!(check.work[0].drop_note, "verified dropped rationale");
    let request = uuid::Uuid::new_v4().to_string();
    let args = json!({"id":id,"expected_revision":check.close_revision.unwrap(),"request_id":request,"note":"captain reviewed the unsuccessful work"});
    let first = captain_socket
        .call("agent.call", tool("todo_ruling_close", args.clone()))
        .await
        .unwrap();
    assert_eq!(first["isError"], false, "{first}");
    let receipt: crewd_proto::RequirementRulingReceipt =
        serde_json::from_str(first["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt.captain.as_deref(), Some(captain.as_str()));
    assert_eq!(receipt.request_id, request);
    f.state.tokens.revoke(&captain);
    let denied = captain_socket
        .call("agent.call", tool("todo_ruling_close", args.clone()))
        .await;
    assert!(denied.is_err() || denied.unwrap()["isError"] == true);
    captain_socket.close().await;
    f.reopen().await;
    let mut human = Socket::owned(f.state.clone());
    let result = human
        .call("todo.rulingResult", json!({"crewId":f.crew,"id":id}))
        .await
        .unwrap();
    assert_eq!(result["receipt"], json!(receipt));
    let final_detail = human.call("todo.get", json!({"id":id})).await.unwrap();
    assert_eq!(final_detail["status"], "closed");
    f.state.admission.allow(&captain);
    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    assert_eq!(
        captain_socket
            .call("agent.call", tool("todo_ruling_close", args))
            .await
            .unwrap(),
        first
    );
    captain_socket.close().await;
    human.close().await;
    f.close().await;
}
