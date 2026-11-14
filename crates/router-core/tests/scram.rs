use router_core::drivers::huawei::scram::ScramClient;
use router_core::RouterError;
use serde_json::Value;

fn vectors() -> Vec<Value> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/scram_vectors.json"
    );
    serde_json::from_str::<Vec<Value>>(&std::fs::read_to_string(p).unwrap()).unwrap()
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap()
}
fn tag(xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let a = xml.find(&open).unwrap() + open.len();
    let b = xml[a..].find(&format!("</{name}>")).unwrap();
    xml[a..a + b].to_string()
}
fn fixture(n: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{n}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn matches_reference_vectors() {
    for v in vectors() {
        let c = ScramClient::with_nonce(s(&v, "first_nonce").to_string());
        let p = c
            .prove(
                s(&v, "password"),
                s(&v, "salt"),
                v["iterations"].as_u64().unwrap() as u32,
                s(&v, "server_nonce"),
            )
            .unwrap();
        assert_eq!(
            p.client_proof_hex,
            s(&v, "clientproof"),
            "{}",
            s(&v, "name")
        );
        assert_eq!(p.final_nonce, s(&v, "server_nonce"));
        p.verify_server(s(&v, "serversignature")).unwrap();
    }
}

#[test]
fn rejects_wrong_server_signature() {
    let v = &vectors()[0];
    let c = ScramClient::with_nonce(s(v, "first_nonce").to_string());
    let p = c
        .prove(s(v, "password"), s(v, "salt"), 1000, s(v, "server_nonce"))
        .unwrap();
    let mut sig = s(v, "serversignature").to_string();
    let flipped = if sig.starts_with('0') { "1" } else { "0" };
    sig.replace_range(0..1, flipped);
    assert!(matches!(
        p.verify_server(&sig),
        Err(RouterError::LoginAlgorithmMismatch)
    ));
    assert!(matches!(
        p.verify_server("zz"),
        Err(RouterError::LoginAlgorithmMismatch)
    ));
}

#[test]
fn rejects_foreign_server_nonce() {
    let v = &vectors()[0];
    let c = ScramClient::with_nonce(s(v, "first_nonce").to_string());
    let r = c.prove(s(v, "password"), s(v, "salt"), 1000, "deadbeefnotmine");
    assert!(matches!(r, Err(RouterError::LoginAlgorithmMismatch)));
}

#[test]
fn new_nonce_is_64_lower_hex() {
    let c = ScramClient::new();
    assert_eq!(c.first_nonce().len(), 64);
    assert!(c
        .first_nonce()
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
}

#[test]
fn debug_hides_signature() {
    let v = &vectors()[0];
    let c = ScramClient::with_nonce(s(v, "first_nonce").to_string());
    let p = c
        .prove(s(v, "password"), s(v, "salt"), 1000, s(v, "server_nonce"))
        .unwrap();
    assert!(!format!("{p:?}").contains(s(v, "serversignature")));
}

#[test]
fn fixture_relations_hold() {
    let first = tag(&fixture("challenge_login_request.xml"), "firstnonce");
    let server = tag(&fixture("challenge_login_response.xml"), "servernonce");
    let fin = tag(&fixture("authentication_login_request.xml"), "finalnonce");
    assert!(server.starts_with(&first));
    assert_eq!(fin, server);
}
