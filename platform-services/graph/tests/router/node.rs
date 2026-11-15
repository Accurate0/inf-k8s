use insta::assert_yaml_snapshot;

use crate::setup::Stack;

#[tokio::test(flavor = "multi_thread")]
async fn global_ids_resolve_through_the_owning_subgraph() {
    let stack = Stack::spawn().await;

    let response = stack
        .post(
            Some(&stack.user()),
            "{ node(id: \"open:thing:42\") { __typename id ... on OpenThing { name } } }",
        )
        .await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_malformed_ids_resolve_to_null() {
    let stack = Stack::spawn().await;

    let response = stack
        .post(
            Some(&stack.user()),
            "{ unknown: node(id: \"open:widget:1\") { id } malformed: node(id: \"nope\") { id } }",
        )
        .await;
    assert_yaml_snapshot!(response.snapshot());
}
