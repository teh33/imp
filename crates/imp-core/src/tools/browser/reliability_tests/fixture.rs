use super::*;

struct FixtureServer {
    address: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl FixtureServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_fixture(stream));
            }
        });
        Self { address, task }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_fixture(mut stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut request = [0_u8; 4096];
    let read = stream.read(&mut request).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, headers, body) = fixture_response(path);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

fn fixture_response(path: &str) -> (&'static str, &'static str, &'static str) {
    match path {
        "/redirect" => ("302 Found", "Location: /form\r\n", "redirect"),
        "/form" => (
            "200 OK",
            "",
            r#"<!doctype html><title>Fixture Form</title>
<form id="fixture-form"><label>Name <input id="name" value=""></label>
<select id="color"><option>red</option><option>blue</option></select>
<label><input id="agree" type="checkbox">Agree</label>
<button id="submit" type="submit">Submit</button></form>
<div id="result"></div><script>
document.querySelector('#fixture-form').addEventListener('submit', event => {
 event.preventDefault(); document.querySelector('#result').textContent = 'submitted:' + document.querySelector('#name').value;
});
</script>"#,
        ),
        "/dynamic" => (
            "200 OK",
            "",
            r#"<!doctype html><title>Dynamic</title><button id="replace">Replace</button>
<script>document.querySelector('#replace').onclick = event => { event.target.outerHTML = '<button id="new-button">New</button>'; };</script>"#,
        ),
        "/large" => (
            "200 OK",
            "",
            "<!doctype html><title>Large</title><main>bounded fixture</main>",
        ),
        _ => (
            "200 OK",
            "",
            "<!doctype html><title>Fixture Home</title><a href='/redirect'>Form</a><a href='/dynamic'>Dynamic</a>",
        ),
    }
}

#[tokio::test]
#[ignore = "requires LIGHTPANDA_BIN and loopback access"]
async fn real_lightpanda_deterministic_fixture() {
    let binary = std::env::var_os("LIGHTPANDA_BIN").expect("set LIGHTPANDA_BIN");
    let fixture = FixtureServer::start().await;
    let dir = TempDir::new().unwrap();
    let tool = BrowserTool::new(BrowserConfig {
        binary: Some(binary.into()),
        timeout_ms: 10_000,
        block_private_networks: false,
        ..Default::default()
    });
    let ctx = context(dir.path());
    let started = start(&tool, ctx.clone()).await;
    assert!(!started.is_error, "{}", text(&started));
    let id = session_id(&started).to_string();
    let navigated = tool
        .execute(
            "navigate",
            json!({"action": "navigate", "session_id": id, "url": fixture.url("/redirect")}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(!navigated.is_error, "{}", text(&navigated));
    let current_url = tool
        .execute(
            "get-url",
            json!({"action": "get_url", "session_id": id}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(
        text(&current_url).contains("/form"),
        "{}",
        text(&current_url)
    );

    let observed = tool
        .execute(
            "observe",
            json!({"action": "observe", "session_id": id}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(!observed.is_error, "{}", text(&observed));
    assert!(text(&observed).contains("Submit"));
    let element_count = observed.details["interactive_elements"]
        .as_u64()
        .unwrap_or(0);
    assert!(element_count >= 1, "{}", text(&observed));

    let markdown = tool
        .execute(
            "markdown",
            json!({"action": "markdown", "session_id": id, "max_bytes": 4096}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(!markdown.is_error, "{}", text(&markdown));
    assert!(text(&markdown).contains("Submit"), "{}", text(&markdown));
    assert_eq!(markdown.details["sequence"], 4);

    let stopped = tool
        .execute("stop", json!({"action": "stop", "session_id": id}), ctx)
        .await
        .unwrap();
    assert!(!stopped.is_error, "{}", text(&stopped));
}
