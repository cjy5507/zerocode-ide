//! What to raise, and what raising it closes.
//!
//! The lens draws every advisory as its own triangle, which answers "what is
//! wrong" and leaves "where do I go" to a person pressing 177 of them. This
//! module turns the same graph the lens drew into the other reading: for each
//! advisory, the DIRECT dependency a workspace member would raise to be rid of
//! it, and for each such dependency, everything raising it closes.
//!
//! Nothing here fetches, caches or writes. It is a pure reading of
//! [`SupplyGraph`], so the report a person exports and the picture they were
//! looking at can never disagree — and a test can hold the whole thing without
//! a lockfile or a network.
//!
//! # What counts as "the thing to raise"
//!
//! A workspace member's own line in its lockfile is the only line a person can
//! edit. So the answer is never "raise `websocket-driver`" — that package is
//! nobody's declared dependency — it is "raise `react-scripts`, which is how
//! `websocket-driver` got here". The walk is the graph's own: from the
//! affected component back along `depends_on` until it reaches a component a
//! member depends on directly. Several such dependencies can reach one
//! component and all of them are named, because raising any one of them may
//! not be enough.
//!
//! An affected component that IS a member is its own answer: the code is the
//! workspace's, and the report says so rather than inventing a dependency to
//! raise.
//!
//! # What this does not judge
//!
//! Whether a package reaches the product's users. The manifest cannot say it:
//! Create React App declares `react-scripts` — a build tool — under
//! `dependencies`, so "declared as a runtime dependency" and "ships" are
//! different sentences. The report carries what the lockfiles DO say (which
//! member, which lockfile, which section the walk started from) and leaves
//! that judgment to the reader.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use super::{Severity, SupplyEdgeKind, SupplyGraph};

/// One advisory, read as work: what it is, what it reaches here, and what a
/// person would raise to be rid of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAdvisory {
    /// The group's name — `RUSTSEC-…`, else `GHSA-…`.
    pub id: String,
    pub severity: Severity,
    pub score: Option<f64>,
    pub summary: String,
    pub url: String,
    /// The component this row is about, as the graph spells it.
    pub component: String,
    pub name: String,
    pub version: String,
    /// The versions the advisory's records name as fixed, for this component's
    /// package. Empty when the advisory names no fix — which is itself the
    /// answer, and the report says so rather than leaving a blank.
    pub fixed: Vec<String>,
    /// The direct dependencies the affected component arrived through, sorted.
    /// Empty when the component is a member: then `member` is true and the
    /// code to change is the workspace's own.
    pub through: Vec<String>,
    /// The affected component is a workspace member.
    pub member: bool,
    /// The lockfiles that name the affected component, workspace-relative.
    pub lockfiles: Vec<String>,
}

/// One direct dependency, and what raising it closes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRaise {
    /// The component id of the dependency to raise.
    pub component: String,
    pub name: String,
    /// The version the lockfile has now.
    pub version: String,
    /// The advisories that stop reaching this workspace when it is raised
    /// far enough, most severe first.
    pub closes: Vec<String>,
    /// The most severe advisory among them — the row's own rank.
    pub worst: Severity,
    /// The members that depend on it directly, sorted.
    pub members: Vec<String>,
}

/// The whole reading: how many of each severity, what to raise, and every
/// advisory behind those two numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplyReport {
    /// Advisory count per severity, most severe first. A severity nothing
    /// carries is absent rather than zero: a table of zeroes reads as a
    /// finding.
    pub counted: Vec<(Severity, usize)>,
    /// Most advisories closed first, then most severe, then by name.
    pub raise: Vec<ReportRaise>,
    /// Most severe first, then by id, then by component — the lens's own order
    /// for the first two, so the report reads in the picture's order.
    pub advisories: Vec<ReportAdvisory>,
    /// Every lockfile the affected components came from, sorted.
    pub lockfiles: Vec<String>,
}

impl SupplyReport {
    /// Nothing to report — no advisory reaches this workspace.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.advisories.is_empty()
    }
}

/// Read a graph as work.
///
/// One pass over the edges builds both directions of the dependency relation;
/// the walk from each affected component is a breadth-first search back up
/// that reverse relation, which visits each component at most once per
/// advisory and cannot loop on a cycle the lockfile admits.
#[must_use]
pub fn report(graph: &SupplyGraph) -> SupplyReport {
    let count = graph.components.len();
    let mut depends: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut depended_on_by: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut affects: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for edge in &graph.edges {
        let (from, to) = (edge.from as usize, edge.to as usize);
        match edge.kind {
            SupplyEdgeKind::DependsOn => {
                if from < count && to < count {
                    depends[from].push(to);
                    depended_on_by[to].push(from);
                }
            }
            SupplyEdgeKind::Affects => {
                if from < graph.vulnerabilities.len() && to < count {
                    affects.entry(to).or_default().push(from);
                }
            }
        }
    }

    /* The lines a person can edit: what a member declares. A member's own
     * dependency on another member is not one of them — raising it is not a
     * version bump, it is a change to code in this tree. */
    let mut direct: BTreeSet<usize> = BTreeSet::new();
    let mut members_of: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for (at, component) in graph.components.iter().enumerate() {
        if !component.member {
            continue;
        }
        for &to in &depends[at] {
            if graph.components[to].member {
                continue;
            }
            direct.insert(to);
            members_of
                .entry(to)
                .or_default()
                .insert(graph.components[at].id.clone());
        }
    }

    let mut advisories: Vec<ReportAdvisory> = Vec::new();
    let mut closes: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut lockfiles: BTreeSet<String> = BTreeSet::new();
    for (&at, vulns) in &affects {
        let component = &graph.components[at];
        let through = reached_from(at, &depended_on_by, &direct);
        lockfiles.extend(component.lockfiles.iter().cloned());
        for &vuln in vulns {
            let vulnerability = &graph.vulnerabilities[vuln];
            for &hop in &through {
                closes
                    .entry(hop)
                    .or_default()
                    .insert(vulnerability.id.clone());
            }
            advisories.push(ReportAdvisory {
                id: vulnerability.id.clone(),
                severity: vulnerability.severity,
                score: vulnerability.score,
                summary: vulnerability.summary.clone(),
                url: vulnerability.url.clone(),
                component: component.id.clone(),
                name: component.name.clone(),
                version: component.version.clone(),
                fixed: fixed_for(vulnerability, &component.name),
                through: through
                    .iter()
                    .map(|&hop| graph.components[hop].id.clone())
                    .collect(),
                member: component.member,
                lockfiles: component.lockfiles.clone(),
            });
        }
    }
    advisories.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.component.cmp(&right.component))
    });

    let worst_of = |ids: &BTreeSet<String>| {
        graph
            .vulnerabilities
            .iter()
            .filter(|vulnerability| ids.contains(&vulnerability.id))
            .map(|vulnerability| vulnerability.severity)
            .max()
            .unwrap_or(Severity::Unknown)
    };
    let mut raise: Vec<ReportRaise> = closes
        .iter()
        .map(|(&at, ids)| {
            let component = &graph.components[at];
            let mut closes: Vec<String> = ids.iter().cloned().collect();
            closes.sort_by_key(|id| {
                let severity = graph
                    .vulnerabilities
                    .iter()
                    .find(|vulnerability| &vulnerability.id == id)
                    .map_or(Severity::Unknown, |vulnerability| vulnerability.severity);
                (std::cmp::Reverse(severity), id.clone())
            });
            ReportRaise {
                component: component.id.clone(),
                name: component.name.clone(),
                version: component.version.clone(),
                worst: worst_of(ids),
                closes,
                members: members_of
                    .get(&at)
                    .map(|members| members.iter().cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect();
    raise.sort_by(|left, right| {
        right
            .closes
            .len()
            .cmp(&left.closes.len())
            .then_with(|| right.worst.cmp(&left.worst))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut counted: BTreeMap<Severity, usize> = BTreeMap::new();
    for advisory in &advisories {
        *counted.entry(advisory.severity).or_default() += 1;
    }
    let mut counted: Vec<(Severity, usize)> = counted.into_iter().collect();
    counted.sort_by(|left, right| right.0.cmp(&left.0));

    SupplyReport {
        counted,
        raise,
        advisories,
        lockfiles: lockfiles.into_iter().collect(),
    }
}

/// The direct dependencies an affected component arrived through.
///
/// Breadth-first up the reverse relation. A direct dependency is an answer AND
/// a place to stop: what stands above it is a member, and a member's line is
/// the direct dependency itself.
fn reached_from(at: usize, depended_on_by: &[Vec<usize>], direct: &BTreeSet<usize>) -> Vec<usize> {
    let mut found: BTreeSet<usize> = BTreeSet::new();
    if direct.contains(&at) {
        found.insert(at);
    }
    let mut seen: BTreeSet<usize> = BTreeSet::from([at]);
    let mut queue: VecDeque<usize> = VecDeque::from([at]);
    while let Some(node) = queue.pop_front() {
        for &above in &depended_on_by[node] {
            if !seen.insert(above) {
                continue;
            }
            if direct.contains(&above) {
                found.insert(above);
                continue;
            }
            queue.push_back(above);
        }
    }
    found.into_iter().collect()
}

/// The fixed versions an advisory names for one package.
///
/// A record fixes a package, not a workspace: `form-data` and `axios` in the
/// same advisory carry different fixes, and printing both beside one component
/// is how a person raises the wrong line.
fn fixed_for(vulnerability: &super::Vulnerability, name: &str) -> Vec<String> {
    let mut versions: Vec<String> = vulnerability
        .fixed
        .iter()
        .filter(|fix| fix.name == name)
        .map(|fix| fix.version.clone())
        .collect();
    if versions.is_empty() {
        versions = vulnerability
            .fixed
            .iter()
            .map(|fix| fix.version.clone())
            .collect();
    }
    versions.sort();
    versions.dedup();
    versions
}

/// The report as a document.
///
/// English, like every other document this repository generates — the window's
/// own words are the button that asks for it and the line that says where it
/// landed. Pure: the same report renders the same bytes, which is what lets a
/// test hold the whole document.
///
/// `title` names the workspace the reading is of. `now` is the stamp the
/// document carries; the caller owns the clock.
#[must_use]
pub fn markdown(report: &SupplyReport, title: &str, now: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Supply chain — {title}\n\n"));
    out.push_str(&format!("Read {now}."));
    if !report.lockfiles.is_empty() {
        out.push_str(&format!(" Lockfiles: {}.", report.lockfiles.join(", ")));
    }
    out.push_str("\n\n");
    if report.is_clear() {
        out.push_str("No advisory reaches this workspace.\n");
        return out;
    }

    out.push_str("## What is here\n\n| Severity | Advisories |\n|---|---:|\n");
    for (severity, count) in &report.counted {
        out.push_str(&format!("| {} | {count} |\n", severity.as_str()));
    }

    out.push_str("\n## What to raise\n\n");
    out.push_str(
        "One line per dependency a member of this workspace declares. Raising it far enough\n         closes everything listed beside it. A build tool a manifest declares as a runtime\n         dependency reads as one here — the lockfiles cannot tell what ships.\n\n",
    );
    out.push_str("| Raise | At | Closes | Worst |\n|---|---|---:|---|\n");
    for row in &report.raise {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            row.name,
            row.version,
            row.closes.len(),
            row.worst.as_str()
        ));
    }

    out.push_str("\n## Every advisory\n\n");
    for advisory in &report.advisories {
        let fixed = if advisory.fixed.is_empty() {
            "no fix named".to_string()
        } else {
            advisory.fixed.join(", ")
        };
        out.push_str(&format!(
            "- **{}** · {} `{}@{}` → fixed in **{fixed}**\n",
            advisory.severity.as_str(),
            advisory.id,
            advisory.name,
            advisory.version
        ));
        if advisory.member {
            out.push_str("  - this workspace's own code — nothing to raise\n");
        } else if !advisory.through.is_empty() {
            let through: Vec<&str> = advisory
                .through
                .iter()
                .map(|id| id.rsplit('/').next().unwrap_or(id))
                .collect();
            out.push_str(&format!("  - arrived through {}\n", through.join(", ")));
        }
        if !advisory.summary.is_empty() {
            out.push_str(&format!("  - {}\n", advisory.summary));
        }
        out.push_str(&format!("  - {}\n", advisory.url));
    }
    out
}

#[cfg(test)]
mod tests;
