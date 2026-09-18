//! What the report promises: the line a person can edit, never the package
//! that happens to carry the flaw.

use super::*;
use crate::supply_chain::{Component, Ecosystem, Fix, Origin, SupplyEdge, Vulnerability};

/// A component the test spells in one line. `deps` are indices into the same
/// slice the graph is built from, which is what the edges carry anyway.
fn component(name: &str, version: &str, member: bool) -> Component {
    let purl = format!("pkg:npm/{name}@{version}");
    Component {
        id: purl.clone(),
        purl,
        ecosystem: Ecosystem::Npm,
        name: name.to_string(),
        version: version.to_string(),
        origin: if member {
            Origin::Path
        } else {
            Origin::Registry
        },
        source: None,
        member,
        dependencies: Vec::new(),
        lockfiles: vec!["package-lock.json".to_string()],
    }
}

fn vulnerability(id: &str, severity: Severity, fixes: &[(&str, &str)]) -> Vulnerability {
    Vulnerability {
        id: id.to_string(),
        aliases: Vec::new(),
        summary: format!("{id} summary"),
        severity,
        score: None,
        fixed: fixes
            .iter()
            .map(|(name, version)| Fix {
                ecosystem: Ecosystem::Npm,
                name: (*name).to_string(),
                version: (*version).to_string(),
            })
            .collect(),
        informational: None,
        url: format!("https://osv.dev/vulnerability/{id}"),
    }
}

fn depends(from: usize, to: usize) -> SupplyEdge {
    SupplyEdge {
        from: from as u32,
        to: to as u32,
        kind: SupplyEdgeKind::DependsOn,
    }
}

fn affects(vulnerability: usize, component: usize) -> SupplyEdge {
    SupplyEdge {
        from: vulnerability as u32,
        to: component as u32,
        kind: SupplyEdgeKind::Affects,
    }
}

/// app ─ react-scripts ─ ws-driver      (the flaw is two hops down)
///  └─── crypto-js                      (the flaw is on the line itself)
fn graph() -> SupplyGraph {
    SupplyGraph {
        components: vec![
            component("app", "0.1.0", true),
            component("react-scripts", "5.0.1", false),
            component("websocket-driver", "0.7.4", false),
            component("crypto-js", "4.1.1", false),
        ],
        vulnerabilities: vec![
            vulnerability(
                "GHSA-ws",
                Severity::Critical,
                &[("websocket-driver", "0.7.5")],
            ),
            vulnerability("GHSA-cj", Severity::High, &[("crypto-js", "4.2.0")]),
        ],
        edges: vec![
            depends(0, 1),
            depends(0, 3),
            depends(1, 2),
            affects(0, 2),
            affects(1, 3),
        ],
    }
}

/// The answer is the line a person can edit.
///
/// `websocket-driver` is nobody's declared dependency — it arrived under
/// `react-scripts` — so a report that names it has told the reader to edit a
/// line that is not in their lockfile. The walk goes back up until it reaches
/// something a member declared, and stops there.
#[test]
fn an_advisory_names_the_direct_dependency_that_carried_it() {
    let report = report(&graph());

    let deep = report
        .advisories
        .iter()
        .find(|advisory| advisory.name == "websocket-driver")
        .expect("the deep advisory");
    assert_eq!(
        deep.through,
        vec!["pkg:npm/react-scripts@5.0.1".to_string()]
    );
    assert_eq!(
        deep.fixed,
        vec!["0.7.5".to_string()],
        "its own fix, not the other package's"
    );
    assert!(!deep.member);

    /* A direct dependency is its own answer: nothing stands between the
     * person's line and the flaw. */
    let near = report
        .advisories
        .iter()
        .find(|advisory| advisory.name == "crypto-js")
        .expect("the direct advisory");
    assert_eq!(near.through, vec!["pkg:npm/crypto-js@4.1.1".to_string()]);
    assert_eq!(near.fixed, vec!["4.2.0".to_string()]);

    /* Most severe first — the lens's own order, so the document reads in the
     * order the picture was drawn in. */
    assert_eq!(
        report
            .advisories
            .iter()
            .map(|advisory| advisory.id.as_str())
            .collect::<Vec<_>>(),
        vec!["GHSA-ws", "GHSA-cj"]
    );
    assert_eq!(
        report.counted,
        vec![(Severity::Critical, 1), (Severity::High, 1)]
    );
    assert_eq!(report.lockfiles, vec!["package-lock.json".to_string()]);
    assert!(!report.is_clear());
}

/// The other reading: one line, everything it closes.
#[test]
fn a_dependency_carries_everything_that_arrived_under_it() {
    let mut graph = graph();
    /* A second flaw under the same build tool: raising it once closes both,
     * which is the number the row exists to say. */
    graph
        .components
        .push(component("shell-quote", "1.8.0", false));
    graph.vulnerabilities.push(vulnerability(
        "GHSA-sq",
        Severity::Medium,
        &[("shell-quote", "1.8.4")],
    ));
    graph.edges.push(depends(1, 4));
    graph.edges.push(affects(2, 4));

    let report = report(&graph);
    let first = &report.raise[0];
    assert_eq!(first.name, "react-scripts");
    assert_eq!(
        first.closes,
        vec!["GHSA-ws".to_string(), "GHSA-sq".to_string()]
    );
    assert_eq!(
        first.worst,
        Severity::Critical,
        "the row ranks by its worst"
    );
    assert_eq!(first.members, vec!["pkg:npm/app@0.1.0".to_string()]);
    assert_eq!(report.raise[1].name, "crypto-js");
    assert_eq!(report.raise[1].closes, vec!["GHSA-cj".to_string()]);
}

/// A member's own code is not a version bump.
///
/// When the flaw is in something this workspace builds, naming a dependency to
/// raise would be an invention: there is no such line. The row says the
/// component is a member and names nothing to raise.
#[test]
fn a_flaw_in_the_workspaces_own_code_names_nothing_to_raise() {
    let mut graph = graph();
    graph.vulnerabilities.push(vulnerability(
        "GHSA-own",
        Severity::High,
        &[("app", "0.2.0")],
    ));
    graph.edges.push(affects(2, 0));

    let report = report(&graph);
    let own = report
        .advisories
        .iter()
        .find(|advisory| advisory.id == "GHSA-own")
        .expect("the member's advisory");
    assert!(own.member);
    assert!(
        own.through.is_empty(),
        "no line to raise: {:?}",
        own.through
    );
    assert!(
        report.raise.iter().all(|row| row.name != "app"),
        "a member is never something to raise"
    );
}

/// A cycle the lockfile admits must not hang the walk.
#[test]
fn a_dependency_cycle_is_walked_once() {
    let mut graph = graph();
    graph.edges.push(depends(2, 1));

    let report = report(&graph);
    let deep = report
        .advisories
        .iter()
        .find(|advisory| advisory.name == "websocket-driver")
        .expect("the deep advisory");
    assert_eq!(
        deep.through,
        vec!["pkg:npm/react-scripts@5.0.1".to_string()]
    );
}

/// Nothing to report is a sentence, not an empty table.
#[test]
fn a_workspace_no_advisory_reaches_is_clear() {
    let report = report(&SupplyGraph {
        components: vec![component("app", "0.1.0", true)],
        vulnerabilities: Vec::new(),
        edges: Vec::new(),
    });
    assert!(report.is_clear());
    assert!(
        report.counted.is_empty(),
        "a table of zeroes reads as a finding"
    );
    assert!(report.raise.is_empty());
}

/// A rating's word and its wire spelling are one table.
#[test]
fn a_severitys_word_is_its_wire_word() {
    for severity in [
        Severity::Unknown,
        Severity::None,
        Severity::Low,
        Severity::Medium,
        Severity::High,
        Severity::Critical,
    ] {
        assert_eq!(
            serde_json::to_value(severity).expect("a severity"),
            serde_json::json!(severity.as_str())
        );
    }
}

/// The document says what to raise before it says what is wrong.
#[test]
fn the_document_leads_with_the_line_to_edit() {
    let text = markdown(&report(&graph()), "card/admin-fe", "2026-09-18 11:00");
    let raise = text.find("## What to raise").expect("the raise section");
    let every = text
        .find("## Every advisory")
        .expect("the advisory section");
    assert!(raise < every, "what to do comes before what is wrong");
    assert!(text.contains("| `react-scripts` | 5.0.1 | 1 | critical |"));
    assert!(!text.contains("arrived through websocket-driver"));
    assert!(text.contains("fixed in **0.7.5**"));
    assert!(text.contains("https://osv.dev/vulnerability/GHSA-ws"));

    let clear = markdown(
        &report(&SupplyGraph {
            components: vec![component("app", "0.1.0", true)],
            vulnerabilities: Vec::new(),
            edges: Vec::new(),
        }),
        "clean",
        "2026-09-18 11:00",
    );
    assert!(clear.contains("No advisory reaches this workspace."));
    assert!(
        !clear.contains("## What to raise"),
        "nothing to raise, no table"
    );
}
