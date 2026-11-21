//! A valid but very deep scan must plan without exhausting the stack (L1). Kept in its own test
//! binary: on a recursive planner this overflows the stack and aborts the process.

use disk_cleaner_core::{
    plan, Class, Node, PlanOptions, ScanOptionsRecord, ScanResult, Totals, SCHEMA_VERSION,
};

fn chain(len: usize) -> ScanResult {
    let nodes: Vec<Node> = (0..len)
        .map(|i| {
            let mut n = Node::new(format!("/synthetic/chain/n{i}"), Class::LooseCode);
            n.parent = if i == 0 { None } else { Some(i - 1) };
            n.bytes_self = 1;
            n.bytes_total = 1;
            n.newest_mtime = Some(0);
            n
        })
        .collect();
    ScanResult {
        schema_version: SCHEMA_VERSION,
        tool_version: "test".into(),
        generated_at: 0,
        roots: vec![],
        options: ScanOptionsRecord::default(),
        totals: Totals::default(),
        nodes,
        outputs: vec![],
        big_files: vec![],
        issues: vec![],
        warnings: vec![],
        enclosing_repos: vec![],
        scope: "subtree".to_string(),
        coverage: disk_cleaner_core::Coverage {
            complete: true,
            ..Default::default()
        },
    }
}

#[test]
fn a_valid_chain_of_100000_nodes_plans_and_validates() {
    let s = chain(100_000);
    let p = plan(&s, &PlanOptions::new(7, 10 * 365 * 86_400));
    assert_eq!(p.items.len(), 100_000);
    let text = serde_json::to_string(&s).unwrap();
    disk_cleaner_core::scan_from_json(&text).expect("a valid deep chain loads");
}
