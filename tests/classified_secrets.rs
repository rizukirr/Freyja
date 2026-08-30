//! What a classified value does, and what it does not do.
//!
//! Two claims, and they pull in opposite directions: the value must reach the
//! server untouched, and must reach nothing Freyja prints. Both are asserted
//! here so neither can be satisfied by breaking the other.

use freyja::{Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};

mod common;
use common::{ok_response, serve};

fn ask() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::User, "Hi"))
}

#[tokio::test]
async fn a_classified_header_reaches_the_server_intact() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .secret_header("x-acme-passport", "live-passport");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let sent = requests.recv().expect("captured request");
    assert!(sent.contains("live-passport"), "{sent}");
}

#[tokio::test]
async fn a_classified_query_parameter_reaches_the_server_intact() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .secret_query("sig", "live-signature");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let line = requests
        .recv()
        .expect("captured request")
        .lines()
        .next()
        .expect("a request line")
        .to_string();
    assert!(line.contains("sig=live-signature"), "{line}");
}

#[tokio::test]
async fn classification_does_not_disturb_header_precedence() {
    // `secret_header` delegates to `header`, so the later of two same-named
    // entries still wins whichever builder wrote it.
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .header("x-route", "first")
        .secret_header("x-route", "second");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let sent = requests.recv().expect("captured request");
    let values: Vec<&str> = sent
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-route"))
        .map(|(_, value)| value.trim())
        .collect();
    assert_eq!(values, ["second"], "{sent}");
}

#[test]
fn debug_withholds_what_the_caller_classified() {
    // Neither name contains a marker the heuristic knows, which is the point.
    let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
        .header("x-acme-tenant", "engineering")
        .secret_header("x-acme-passport", "live-passport")
        .query("api-version", "2024-02-01")
        .secret_query("sig", "live-signature");

    let printed = format!("{config:?}");

    assert!(!printed.contains("live-passport"), "{printed}");
    assert!(!printed.contains("live-signature"), "{printed}");
    // Configuration stays readable, which is why the default is not inverted.
    assert!(printed.contains("engineering"), "{printed}");
    assert!(printed.contains("2024-02-01"), "{printed}");
}

#[test]
fn debug_still_withholds_an_unclassified_credential_shaped_name() {
    // The backstop: a caller who classifies nothing keeps what they have today.
    let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
        .header("x-api-key", "live-key");

    assert!(!format!("{config:?}").contains("live-key"));
}

#[tokio::test]
async fn a_classified_query_parameter_is_withheld_from_a_transport_error() {
    // Port 1 refuses, so the failure carries the URL it tried. Neither name
    // contains a marker the heuristic knows, so the classification is doing
    // the work.
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", "http://127.0.0.1:1")
        .default_model("test-model")
        .secret_query("sig", "live-signature")
        .query("api-version", "2024-02-01");

    let error = Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect_err("nothing listens on port 1");

    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(!rendered.contains("live-signature"), "{rendered}");
        assert!(rendered.contains("api-version=2024-02-01"), "{rendered}");
    }
}
