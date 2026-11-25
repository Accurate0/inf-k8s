use crate::setup::Stack;

const ME: &str =
    "{ me { subject username kind issuer externalIssuer client scopes subgraphs expiresIn } }";

#[tokio::test(flavor = "multi_thread")]
async fn me_describes_the_verified_caller() {
    let stack = Stack::spawn().await;

    let user = stack
        .post(Some(&stack.scoped("gated:secret:read")), ME)
        .await;
    let me = &user.body["data"]["me"];
    assert_eq!(me["kind"], "USER", "{}", user.body);
    assert_eq!(me["username"], "tester");
    assert_eq!(me["subject"], "00000000-0000-0000-0000-000000000001");
    assert_eq!(me["client"], "graph");
    assert!(me["externalIssuer"].is_null());
    assert!(
        me["scopes"]
            .as_array()
            .unwrap()
            .contains(&"gated:secret:read".into())
    );
    assert_eq!(me["subgraphs"], serde_json::json!(["gated"]));
    assert!(me["expiresIn"].as_i64().unwrap() > 0);

    let plain = stack.post(Some(&stack.user()), ME).await;
    assert_eq!(
        plain.body["data"]["me"]["subgraphs"],
        serde_json::json!([]),
        "{}",
        plain.body
    );

    let ci = stack.forgejo("anurag/k8s", "refs/heads/main");
    let external = stack.post(Some(&ci), ME).await;
    let me = &external.body["data"]["me"];
    assert_eq!(me["kind"], "EXTERNAL", "{}", external.body);
    assert_eq!(me["externalIssuer"], "forgejo");
    assert_eq!(me["scopes"], serde_json::json!(["open:note:read"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_callers_need_a_document_for_me_too() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ me { subject } }").await;
    assert!(response.body["data"]["me"].is_object(), "{}", response.body);

    let anonymous = stack.post(None, "{ me { subject } }").await;
    assert_eq!(
        anonymous.body["errors"][0]["extensions"]["code"],
        "PERSISTED_DOCUMENT_ID_REQUIRED"
    );
}
