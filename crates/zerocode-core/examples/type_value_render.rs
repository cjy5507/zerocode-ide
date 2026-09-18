//! Print what the type-value seat sends, case by case, so the probe that
//! measured the seat's table can be held to the same bytes.
//!
//! The words, the keys and their labels live in one file
//! (`fixtures/type-value/question.json`) that both readers parse, and a source
//! contract holds that `tools/type_value_latency.py` spells none of them
//! itself. This is how the claim is CHECKED rather than asserted — a Rust test
//! cannot run Python, so the two renders are put side by side instead:
//!
//! ```text
//! cargo run -p zerocode-core --example type_value_render > /tmp/rust
//! python3 - <<'PY' > /tmp/probe
//! import importlib.util, json
//! spec = importlib.util.spec_from_file_location('tv', 'tools/type_value_latency.py')
//! tv = importlib.util.module_from_spec(spec); spec.loader.exec_module(tv)
//! q = json.loads(tv.QUESTION_FILE.read_text())
//! for case in q['cases']:
//!     print(f"--{case['id']}--"); print(tv.render(q, case))
//! PY
//! diff /tmp/rust /tmp/probe        # 2026-09-18: identical, six cases
//! ```

use zerocode_core::type_value::{FieldLook, asked, render};

fn main() {
    for case in &asked().cases {
        println!("--{}--", case.id);
        println!(
            "{}",
            render(&FieldLook {
                goal: &case.goal,
                label: &case.field.label,
                placeholder: &case.field.placeholder,
                near: &case.field.near,
            })
        );
    }
}
