//! The M0 exit gate: the example car drives on a flat plane and its headless
//! scenario tests pass, deterministically.

use std::path::Path;
use waypoint::project::Project;
use waypoint::test::run_scenario;

#[test]
fn sandbox_scenarios_pass() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox");
    let (project, diags) = Project::load(&root);
    assert!(!diags.has_errors());
    let project = project.unwrap();
    assert!(!project.scenarios.is_empty());
    for (name, sc) in &project.scenarios {
        let _ = sc;
        let input = project.scenario_input(name).expect("scenario inputs exist");
        let report =
            run_scenario(&input, &|| waypoint::make_app(project.disabled_plugins())).unwrap();
        println!("{}", serde_json_pretty(&report));
        assert!(report.passed, "scenario `{name}` failed");
    }
}

fn serde_json_pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap()
}
