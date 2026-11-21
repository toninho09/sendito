mod common;
use common::Must;
use sendito_testkit::{grpc_echo, http_echo};

#[test]
#[ignore]
fn peek() {
    let rt = tokio::runtime::Runtime::new().must();
    let (http, grpc) = rt.block_on(async {
        (
            http_echo::start().await.must(),
            grpc_echo::start().await.must(),
        )
    });
    let (hu, gt) = (http.url(), grpc.target());
    let mut h = common::Harness::with(100, 30, |d| common::demo_collection(d, &hu, &gt));
    h.app.set_env(Some("local".into()));
    h.key("ctrl+w h");
    h.app.tree.set_filter("health");
    h.key("enter");
    h.app.tree.set_filter("");
    h.key("f5");
    h.wait_idle(10);
    h.key("ctrl+w j");
    println!("focus {:?}", h.app.focus);
    h.key("alt+1");
    h.key("/");
    println!("{}", h.screen());
    h.type_text("status");
    h.key("enter");
    println!("focus {:?}", h.app.focus);
    h.key("x");
    println!("{}", h.screen());
}
