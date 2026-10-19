//! The time zone pages are shown in, environment profiles, and the default window
//! of a new investigation.

use std::{path::Path, sync::Arc};

use kyumei_app::{
    InvestigationService, LocalInvestigations, SystemClock,
    config::Config,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_domain::DisplayZone;
use kyumei_knowledge::KnowledgeBase;
use kyumei_web::AppState;
use topcoat::router::{Body, Method, Router, StatusCode, header, request::Request, to_bytes};

const CONFIG: &str = r#"
time_zones = ["Europe/London", "America/New_York"]

[[profile]]
name = "local-dev"
description = "Temporal dev server on this machine"
server_version = "1.25.1"
namespace = "orders"
task_queue = "fulfil-q"
deployment = "self-hosted"
history_hosts = 1
persistence = "SQLite (dev server)"
"#;

struct Reply {
    status: StatusCode,
    location: String,
    cookie: String,
    body: String,
}

/// A request as a browser with `cookie` would send it.
async fn send(
    router: &Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    form: Option<&str>,
) -> Reply {
    let mut b = Request::<()>::builder().method(method).uri(path);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let body = match form {
        Some(f) => {
            b = b.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(f.to_owned())
        }
        None => Body::default(),
    };
    let response = router.handle(b.body(body).expect("request")).await;
    let (parts, body) = response.into_parts();
    let get = |h| {
        parts
            .headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    Reply {
        status: parts.status,
        location: get(header::LOCATION),
        cookie: get(header::SET_COOKIE),
        body: String::from_utf8_lossy(&to_bytes(body, usize::MAX).await.expect("body"))
            .into_owned(),
    }
}

fn router(service: Arc<dyn InvestigationService>, kb: Arc<KnowledgeBase>) -> Router {
    let config = Config::parse(CONFIG, Path::new("config.toml")).expect("config");
    kyumei_web::router_for(AppState {
        config: Arc::new(config),
        ..AppState::new(kb, service, "test".to_owned())
    })
}

#[tokio::test]
async fn the_zone_is_chosen_once_and_kept_in_a_cookie() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), Arc::new(SystemClock)));
    let router = router(service, kb);

    // UTC by default; the selector offers UTC and the preferred zones only.
    let page = send(&router, Method::GET, "/", None, None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("<option value=\"UTC\" selected"),
        "UTC by default"
    );
    assert!(page.body.contains("<option value=\"Europe/London\""));
    assert!(page.body.contains("<option value=\"America/New_York\""));
    assert!(
        !page.body.contains("<option value=\"Asia/Tokyo\""),
        "only preferred zones"
    );
    assert!(page.body.contains("Incident from (UTC)"));

    // Choosing a zone keeps it in a cookie and goes back where it was chosen.
    let set = send(
        &router,
        Method::POST,
        "/preferences/time-zone",
        None,
        Some("zone=Europe%2FLondon&back=%2Fknowledge"),
    )
    .await;
    assert_eq!(set.status, StatusCode::SEE_OTHER);
    assert_eq!(set.location, "/knowledge");
    assert!(
        set.cookie.starts_with("kyumei_tz=Europe/London;"),
        "{}",
        set.cookie
    );

    let london = DisplayZone::named("Europe/London").expect("zone");
    let label = london.label(jiff::Timestamp::now());
    let page = send(
        &router,
        Method::GET,
        "/",
        Some("kyumei_tz=Europe/London"),
        None,
    )
    .await;
    assert!(
        page.body
            .contains("<option value=\"Europe/London\" selected")
    );
    assert!(
        page.body.contains(&format!("Incident from ({label})")),
        "form fields are read in the zone"
    );

    // A zone the configuration doesn't offer falls back to UTC, and only paths on
    // this site are redirected to.
    let set = send(
        &router,
        Method::POST,
        "/preferences/time-zone",
        None,
        Some("zone=Asia%2FTokyo&back=https%3A%2F%2Fexample.com%2F"),
    )
    .await;
    assert!(set.cookie.starts_with("kyumei_tz=UTC;"), "{}", set.cookie);
    assert_eq!(set.location, "/");
    let page = send(
        &router,
        Method::GET,
        "/",
        Some("kyumei_tz=Asia/Tokyo"),
        None,
    )
    .await;
    assert!(page.body.contains("<option value=\"UTC\" selected"));
}

#[tokio::test]
async fn a_profile_and_the_last_hour_fill_a_new_investigation() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), Arc::new(SystemClock)));
    let router = router(service.clone(), kb);

    // The window starts as the last hour.
    let utc = DisplayZone::utc();
    let now = jiff::Timestamp::now();
    let page = send(&router, Method::GET, "/", None, None).await;
    let hour_ago = utc.input_value(now - jiff::SignedDuration::from_hours(1));
    let to = utc.input_value(now);
    assert!(
        page.body.contains(&format!(
            "name=\"window_start\" value=\"{}",
            &hour_ago[..13]
        )),
        "from an hour ago"
    );
    assert!(
        page.body
            .contains(&format!("name=\"window_end\" value=\"{}", &to[..13])),
        "to now"
    );
    assert!(
        page.body.contains("href=\"/?profile=local-dev#new\""),
        "profiles are offered"
    );

    // A profile fills the context, and the investigation records it.
    let page = send(&router, Method::GET, "/?profile=local-dev", None, None).await;
    assert!(
        page.body
            .contains("name=\"namespace\" spellcheck=\"false\" value=\"orders\"")
    );
    assert!(
        page.body
            .contains("<option value=\"self_hosted_other\" selected")
    );
    assert!(page.body.contains("name=\"profile\" value=\"local-dev\""));
    let opened = send(
        &router,
        Method::POST,
        "/investigations",
        None,
        Some(
            "description=Workflows+in+orders+show+up+late+in+listings&profile=local-dev\
             &server_version=1.25.1&namespace=orders&task_queue=fulfil-q\
             &deployment=self_hosted_other&history_hosts=1\
             &window_start=2026-10-14T13%3A03&window_end=2026-10-14T14%3A03",
        ),
    )
    .await;
    assert_eq!(opened.status, StatusCode::SEE_OTHER, "{}", opened.location);
    let id = opened
        .location
        .trim_start_matches("/investigations/")
        .to_owned();
    let inv = service
        .snapshot(&id.as_str().into())
        .await
        .expect("snapshot")
        .investigation;
    assert_eq!(inv.environment.profile.as_deref(), Some("local-dev"));
    assert_eq!(
        inv.window.start,
        Some("2026-10-14T13:03:00Z".parse().expect("ts"))
    );
    let page = send(&router, Method::GET, &opened.location, None, None).await;
    assert!(page.body.contains(">Profile<"));
    assert!(page.body.contains("local-dev"));
}

#[tokio::test]
async fn an_investigation_reads_in_the_chosen_zone() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx =
        Fixture::load(&fixture::fixtures_dir().join("visibility-es-slow-writes")).expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let local = Arc::new(LocalInvestigations::new(kb.clone(), clock.clone()));
    let (id, _) = fixture::replay(&kb, local.as_ref(), &fx, Some(&clock), true)
        .await
        .expect("replay");
    let service: Arc<dyn InvestigationService> = local;
    let router = router(service, kb);
    let path = format!("/investigations/{id}");

    let utc = send(&router, Method::GET, &path, None, None).await;
    assert!(
        utc.body.contains("2026-10-16 10:00–10:40 UTC"),
        "the window in UTC"
    );

    let cookie = Some("kyumei_tz=America/New_York");
    let page = send(&router, Method::GET, &path, cookie, None).await;
    assert!(
        page.body.contains("2026-10-16 06:00–06:40 EDT"),
        "the window in New York"
    );
    assert!(
        page.body.contains("times in EDT"),
        "the trail names its zone"
    );
    assert!(
        page.body.contains("<time title=\"2026-10-16 ") && page.body.contains(" UTC\">"),
        "hovering a time shows it in UTC"
    );
    assert!(
        page.body
            .contains("name=\"window_start\" value=\"2026-10-16T06:00\""),
        "the context form shows the window in the zone"
    );
    // Request windows Kyumei wrote in UTC read in the zone too.
    assert!(
        !page.body.contains(" UTC (the incident window"),
        "request windows are localized"
    );

    // Submitting the form unchanged changes nothing, though it is written to the minute.
    let unchanged = send(
        &router,
        Method::POST,
        &format!("{path}/context"),
        cookie,
        Some("window_start=2026-10-16T06%3A00&window_end=2026-10-16T06%3A40"),
    )
    .await;
    assert_eq!(unchanged.status, StatusCode::SEE_OTHER);
    let page = send(&router, Method::GET, &path, cookie, None).await;
    assert!(!page.body.contains("Context amended"));

    let export = send(
        &router,
        Method::GET,
        &format!("{path}/export.md"),
        cookie,
        None,
    )
    .await;
    assert!(
        export.body.contains("| Time (EDT) | Event | Refs |"),
        "the export names its zone"
    );
}
