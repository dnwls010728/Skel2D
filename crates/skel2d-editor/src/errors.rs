//! Plain words for edits the core refused. The core's messages stay as they are
//! for agents and the CLI; anything not listed here is shown unchanged.

use crate::i18n::tr;

const CROSSES: &str = "the mesh outline would cross itself. Move the point a shorter way.";
const FLIPS: &str = "the mesh outline would flip or collapse. Move the point back toward the mesh.";
const OUTSIDE: &str = "a point would end up outside the mesh outline. Keep it inside, or move the outline first.";
const FOLDS_CROSS: &str = "fold lines would cross, or two points would land on the same spot. Move the point a little further away.";
const FOLD_LEAVES: &str = "a fold line would leave the mesh. Remove that fold line, or move the point less far.";
const NO_FIT: &str = "the triangles no longer fit the outline. Use Rebuild mesh.";
const ADD_INSIDE: &str = "click inside the mesh to add a point.";
const ADD_TAKEN: &str = "a point is already there. Click somewhere else.";

const REFUSALS: [(&str, &str); 11] = [
    ("hull crosses itself", CROSSES),
    ("hull must have nonzero area and clockwise winding", FLIPS),
    ("vertex is outside the stored hull", OUTSIDE),
    ("vertex lies outside the mesh boundary", OUTSIDE),
    ("crossing constraints or duplicate vertex positions", FOLDS_CROSS),
    ("constraint is not an edge of the mesh triangles", FOLD_LEAVES),
    ("constrained edge leaves the mesh boundary", FOLD_LEAVES),
    ("triangle lies outside the stored hull", NO_FIT),
    ("hull segment is not an edge of the mesh triangles", NO_FIT),
    ("point lies outside the mesh", ADD_INSIDE),
    ("point coincides with an existing vertex", ADD_TAKEN),
];

/// What to show for a refused edit, naming the mesh when the error does.
pub fn describe(error: &str) -> String {
    let Some((_, reason)) = REFUSALS.iter().find(|(core, _)| error.contains(core)) else { return error.to_string() };
    let mesh = error.split(".attachments[").nth(1).and_then(|rest| rest.split(']').next()).unwrap_or(tr("Mesh"));
    crate::tf!("Not applied ({}): {}", mesh, tr(reason))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use skel2d_core::{apply, from_str, parse_ops};

    use super::*;

    #[test]
    fn real_core_refusals_get_plain_words() {
        let vertex = |x: f32, y: f32| json!({"x":x,"y":y,"u":0,"v":0});
        let points = [[0.0, 0.0], [0.0, 10.0], [4.0, 10.0], [5.0, 5.0], [6.0, 10.0], [10.0, 10.0], [10.0, 0.0], [2.0, 2.0]];
        let cloth = json!({"type":"mesh","name":"cloth","hull":7,"vertices":points.map(|p| vertex(p[0], p[1])),"triangles":[[0,1,7],[1,2,7],[2,3,7],[0,7,3],[0,3,6],[3,4,5],[3,5,6]]});
        let doc = from_str(&json!({"bones":[{"name":"root"}],"slots":[{"name":"body","bone":"root","attachment":"cloth","attachments":[cloth]}]}).to_string()).unwrap();
        // Tangling is allowed; the reason is what a warning and a rebuild refusal show.
        let problem = |index: usize, x: f32, y: f32| {
            let ops = json!([{"op":"set_vertices","slot":"body","attachment":"cloth","vertices":[{"index":index,"x":x,"y":y}]}]);
            let moved = apply(&doc, &parse_ops(&ops.to_string()).unwrap()).unwrap();
            let skel2d_core::Attachment::Mesh(mesh) = &moved.slots[0].attachments[0] else { panic!() };
            describe(&skel2d_core::mesh::geometry_problem(mesh).unwrap())
        };
        for (index, x, y, reason) in [(2, 9.0, 9.0, CROSSES), (7, -5.0, 2.0, OUTSIDE), (7, 0.0, 0.0, FOLDS_CROSS)] {
            let shown = problem(index, x, y);
            assert!(shown.contains(tr(reason)), "{shown}");
        }
    }

    #[test]
    fn unknown_errors_are_shown_as_they_are() {
        assert_eq!(describe("bone \"x\" does not exist"), "bone \"x\" does not exist");
    }

    #[test]
    fn every_reason_is_translated() {
        for reason in REFUSALS.map(|(_, reason)| reason).into_iter().chain(["Not applied ({}): {}"]) {
            assert!(crate::i18n::KO_TABLES.iter().any(|t| t.iter().any(|(k, _)| *k == reason)), "{reason}");
        }
    }
}
