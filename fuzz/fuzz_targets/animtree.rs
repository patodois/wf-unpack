#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::animtree::{Fsm, MovementSet, Tree};

fuzz_target!(|data: &[u8]| {
    let [head, body] = wf_unpack_fuzz::split::<2>(data);
    let _ = Fsm::parse(head, body);
    let _ = MovementSet::parse(head);
    if let Ok(t) = Tree::parse(head, body) {
        for start in 0..t.nodes.len().min(8) {
            let _ = t.leaves(start, None);
            let _ = t.leaves(start, Some("Stance"));
        }
    }
});
