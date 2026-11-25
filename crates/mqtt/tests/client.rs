use std::time::Duration;

use sendito_mqtt::{BrokerOptions, Event, Protocol, Publish, Session, Subscription, Will, open};
use sendito_testkit::mqtt_broker::{MqttBroker, wait_for};

fn options(broker: &MqttBroker, client_id: &str) -> BrokerOptions {
    BrokerOptions {
        host: "127.0.0.1".into(),
        port: broker.addr.port(),
        tls: false,
        client_id: client_id.into(),
        username: None,
        password: None,
        protocol: Protocol::V3_1_1,
        clean_session: true,
        keep_alive_s: 30,
        verify_tls: true,
        tls_material: Default::default(),
        last_will: None,
    }
}

async fn next(session: &mut Session) -> Event {
    match tokio::time::timeout(Duration::from_secs(3), session.events.recv()).await {
        Ok(Some(event)) => event,
        Ok(None) => panic!("event channel closed"),
        Err(_) => panic!("event timeout"),
    }
}

#[tokio::test]
async fn connects_subscribes_and_publishes_in_order() {
    let broker = MqttBroker::start().await.unwrap();
    let mut session = open(
        options(&broker, "c1"),
        vec![Subscription {
            topic: "a/b".into(),
            qos: 1,
        }],
        vec![Publish {
            topic: "a/b".into(),
            payload: b"hello".to_vec(),
            qos: 1,
            retain: false,
        }],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    assert!(matches!(
        next(&mut session).await,
        Event::Connected {
            session_present: false,
            ..
        }
    ));
    assert!(matches!(
        next(&mut session).await,
        Event::Subscribed { ref topic, qos: 1, .. } if topic == "a/b"
    ));
    assert!(matches!(next(&mut session).await, Event::Published { .. }));
    match next(&mut session).await {
        Event::Message {
            topic,
            payload,
            qos,
            ..
        } => {
            assert_eq!(topic, "a/b");
            assert_eq!(payload, b"hello");
            assert_eq!(qos, 1);
        }
        other => panic!("unexpected {other:?}"),
    }
    session.disconnect();
    let captures = wait_for(&broker, |c| !c.is_empty()).await;
    let dump = captures[0].dump();
    let connect = dump.find("< CONNECT").unwrap();
    let subscribe = dump.find("< SUBSCRIBE").unwrap();
    let publish = dump.find("< PUBLISH").unwrap();
    assert!(connect < subscribe && subscribe < publish, "{dump}");
    assert!(dump.contains("client-id c1"), "{dump}");
}

#[tokio::test]
async fn live_publish_and_qos_two() {
    let broker = MqttBroker::start().await.unwrap();
    let mut session = open(
        options(&broker, "c2"),
        vec![Subscription {
            topic: "s/#".into(),
            qos: 2,
        }],
        vec![],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    next(&mut session).await;
    next(&mut session).await;
    session
        .publish(Publish {
            topic: "s/x".into(),
            payload: b"q2".to_vec(),
            qos: 2,
            retain: true,
        })
        .unwrap();
    assert!(matches!(
        next(&mut session).await,
        Event::Published { qos: 2, .. }
    ));
    match next(&mut session).await {
        Event::Message { payload, qos, .. } => {
            assert_eq!(payload, b"q2");
            assert_eq!(qos, 2);
        }
        other => panic!("unexpected {other:?}"),
    }
    session.disconnect();
    assert_eq!(
        broker.retained("s/x"),
        Some((b"q2".to_vec(), 2u8)),
        "the broker keeps the retained message"
    );
}

#[tokio::test]
async fn retained_message_arrives_at_subscribe() {
    let broker = MqttBroker::start().await.unwrap();
    let mut first = open(
        options(&broker, "c3"),
        vec![],
        vec![Publish {
            topic: "state/one".into(),
            payload: b"on".to_vec(),
            qos: 1,
            retain: true,
        }],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    next(&mut first).await;
    next(&mut first).await;
    first.disconnect();

    let mut second = open(
        options(&broker, "c4"),
        vec![Subscription {
            topic: "state/+".into(),
            qos: 1,
        }],
        vec![],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    next(&mut second).await;
    next(&mut second).await;
    match next(&mut second).await {
        Event::Message {
            topic,
            payload,
            retain,
            ..
        } => {
            assert_eq!(topic, "state/one");
            assert_eq!(payload, b"on");
            assert!(retain);
        }
        other => panic!("unexpected {other:?}"),
    }
    second.disconnect();
}

#[tokio::test]
async fn version_five_reports_the_same_events() {
    let broker = MqttBroker::start().await.unwrap();
    let mut opts = options(&broker, "c5");
    opts.protocol = Protocol::V5;
    let mut session = open(
        opts,
        vec![Subscription {
            topic: "v5/t".into(),
            qos: 1,
        }],
        vec![Publish {
            topic: "v5/t".into(),
            payload: b"five".to_vec(),
            qos: 1,
            retain: false,
        }],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    next(&mut session).await;
    next(&mut session).await;
    next(&mut session).await;
    match next(&mut session).await {
        Event::Message { payload, .. } => assert_eq!(payload, b"five"),
        other => panic!("unexpected {other:?}"),
    }
    session.disconnect();
    let captures = wait_for(&broker, |c| !c.is_empty()).await;
    let dump = captures[0].dump();
    assert!(dump.starts_with("protocol 5"), "{dump}");
}

#[tokio::test]
async fn refused_connection_ends_with_the_reason() {
    let broker = MqttBroker::start().await.unwrap();
    let mut opts = options(&broker, "c6");
    opts.username = Some("bad".into());
    opts.password = Some("x".into());
    let mut session = open(opts, vec![], vec![], None).await.unwrap();
    let event = next(&mut session).await;
    match event {
        Event::Ended { reason, error, .. } => {
            assert_eq!(reason, "error");
            assert!(
                error.as_ref().is_some_and(|e| e.contains("refused")
                    || e.contains("Refused")
                    || e.contains("refuse")),
                "{error:?}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn last_will_goes_out_when_the_client_drops() {
    let broker = MqttBroker::start().await.unwrap();
    let mut watcher = open(
        options(&broker, "watch"),
        vec![Subscription {
            topic: "will/t".into(),
            qos: 1,
        }],
        vec![],
        Some(Duration::from_secs(5)),
    )
    .await
    .unwrap();
    next(&mut watcher).await;
    next(&mut watcher).await;

    let mut opts = options(&broker, "dying");
    opts.last_will = Some(Will {
        topic: "will/t".into(),
        payload: b"gone".to_vec(),
        qos: 1,
        retain: false,
    });
    let mut dying = open(opts, vec![], vec![], None).await.unwrap();
    next(&mut dying).await;
    dying.cancel();

    match next(&mut watcher).await {
        Event::Message { topic, payload, .. } => {
            assert_eq!(topic, "will/t");
            assert_eq!(payload, b"gone");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn deadline_ends_the_session() {
    let broker = MqttBroker::start().await.unwrap();
    let mut session = open(
        options(&broker, "c7"),
        vec![],
        vec![],
        Some(Duration::from_millis(120)),
    )
    .await
    .unwrap();
    assert!(matches!(next(&mut session).await, Event::Connected { .. }));
    match next(&mut session).await {
        Event::Ended { reason, .. } => assert_eq!(reason, "deadline"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn validation_rejects_bad_values_before_connecting() {
    let broker = MqttBroker::start().await.unwrap();
    let mut opts = options(&broker, "c8");
    opts.port = 0;
    assert!(open(opts, vec![], vec![], None).await.is_err());
    assert!(
        open(
            options(&broker, "c9"),
            vec![Subscription {
                topic: "".into(),
                qos: 0
            }],
            vec![],
            None
        )
        .await
        .is_err()
    );
    assert!(
        open(
            options(&broker, "c10"),
            vec![],
            vec![Publish {
                topic: "a/+".into(),
                payload: vec![],
                qos: 0,
                retain: false
            }],
            None
        )
        .await
        .is_err()
    );
    assert!(
        open(
            options(&broker, "c11"),
            vec![Subscription {
                topic: "a".into(),
                qos: 3
            }],
            vec![],
            None
        )
        .await
        .is_err()
    );
    assert_eq!(Protocol::parse("5").unwrap(), Protocol::V5);
    assert!(Protocol::parse("4").is_err());
    assert_eq!(Protocol::V5.as_str(), "5");
    assert_eq!(
        Event::Ended {
            reason: "x".into(),
            error: None,
            at: Duration::ZERO
        }
        .label(),
        "ended"
    );
}

#[tokio::test]
async fn more_requests_than_the_client_queue_holds_do_not_hang() {
    // The client queue holds 64 requests: 70 subscriptions and 70 initial
    // publishes must all go out.
    let broker = MqttBroker::start().await.unwrap();
    let subscriptions = (0..70)
        .map(|i| Subscription {
            topic: format!("many/{i}"),
            qos: 0,
        })
        .collect();
    let initial = (0..70)
        .map(|i| Publish {
            topic: format!("many/{i}"),
            payload: b"x".to_vec(),
            qos: 0,
            retain: false,
        })
        .collect();
    let mut session = open(
        options(&broker, "c-many"),
        subscriptions,
        initial,
        Some(Duration::from_secs(5)),
    )
    .await
    .unwrap();
    let (mut subscribed, mut published) = (0, 0);
    while subscribed < 70 || published < 70 {
        match next(&mut session).await {
            Event::Subscribed { .. } => subscribed += 1,
            Event::Published { .. } => published += 1,
            Event::Ended { reason, error, .. } => panic!("ended early: {reason} {error:?}"),
            _ => {}
        }
    }
    session.disconnect();
}

#[tokio::test]
async fn a_refused_subscription_is_reported() {
    let broker = MqttBroker::start().await.unwrap();
    let mut session = open(
        options(&broker, "c-denied"),
        vec![
            Subscription {
                topic: "denied/x".into(),
                qos: 1,
            },
            Subscription {
                topic: "ok/x".into(),
                qos: 1,
            },
        ],
        vec![],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
    assert!(matches!(next(&mut session).await, Event::Connected { .. }));
    assert!(matches!(
        next(&mut session).await,
        Event::SubscribeRefused { ref topic, .. } if topic == "denied/x"
    ));
    assert!(matches!(
        next(&mut session).await,
        Event::Subscribed { ref topic, .. } if topic == "ok/x"
    ));
    session.disconnect();
}

#[tokio::test]
async fn an_invalid_topic_filter_is_rejected_before_the_connection() {
    let broker = MqttBroker::start().await.unwrap();
    let error = open(
        options(&broker, "c-bad-filter"),
        vec![Subscription {
            topic: "a/b#".into(),
            qos: 0,
        }],
        vec![],
        None,
    )
    .await
    .err()
    .unwrap();
    assert!(error.contains("not valid"), "{error}");
}
