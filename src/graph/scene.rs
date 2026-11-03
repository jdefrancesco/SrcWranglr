//! A laid-out graph, in world coordinates: Sourcetrail-style *pills* (one per
//! symbol), *groups* (rounded boxes such as a class holding its member
//! pills), and *wires* (relationships, drawn between pill edges).
//!
//! Builders (`trail`, `overview`) produce a `Scene`; `render` paints it and
//! the view hit-tests it. Nothing here depends on egui's runtime.

use egui::{Pos2, Rect, Vec2};

use crate::model::{EdgeKind, SymbolId, SymbolKind};

pub const PILL_HEIGHT: f32 = 22.0;
pub const PILL_PADDING_X: f32 = 10.0;
pub const PILL_GAP: f32 = 6.0;
pub const MIN_PILL_WIDTH: f32 = 36.0;
pub const GROUP_PADDING: f32 = 10.0;
pub const GROUP_TITLE_HEIGHT: f32 = 28.0;
/// Space for the "+N more" footer of truncated groups.
pub const GROUP_FOOTER_HEIGHT: f32 = 18.0;
pub const BLOCK_GAP: f32 = 18.0;

/// What a piece of text is used for; builders measure text through this so
/// layout stays testable without real fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    Pill,
    Title,
}

/// Text width in world units at zoom 1.
pub type Measure<'a> = &'a dyn Fn(&str, TextRole) -> f32;

#[derive(Debug, Clone, PartialEq)]
pub struct Pill {
    pub id: SymbolId,
    pub label: String,
    pub kind: SymbolKind,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The symbol the box stands for (a class), or `None` for a plain
    /// container such as a file in the overview.
    pub id: Option<SymbolId>,
    pub title: String,
    pub kind: Option<SymbolKind>,
    pub rect: Rect,
    pub title_rect: Rect,
    pub pills: Vec<Pill>,
    /// Members left out to keep the box small.
    pub hidden_members: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wire {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub active: Option<SymbolId>,
    pub groups: Vec<Group>,
    /// Pills not inside any group (free functions, etc.).
    pub pills: Vec<Pill>,
    pub wires: Vec<Wire>,
    /// Related symbols left out because of the neighbor cap.
    pub hidden_neighbors: usize,
}

impl Scene {
    pub fn all_pills(&self) -> impl Iterator<Item = &Pill> {
        self.pills
            .iter()
            .chain(self.groups.iter().flat_map(|g| &g.pills))
    }

    /// Where wires to `id` attach: its pill, or the title of its group.
    pub fn anchor(&self, id: SymbolId) -> Option<Rect> {
        self.all_pills()
            .find(|p| p.id == id)
            .map(|p| p.rect)
            .or_else(|| {
                self.groups
                    .iter()
                    .find(|g| g.id == Some(id))
                    .map(|g| g.title_rect)
            })
    }

    /// The symbol under `world`: a pill, else a group title/box with an id.
    pub fn hit(&self, world: Pos2) -> Option<SymbolId> {
        self.all_pills()
            .find(|p| p.rect.contains(world))
            .map(|p| p.id)
            .or_else(|| {
                self.groups
                    .iter()
                    .find(|g| g.rect.contains(world))
                    .and_then(|g| g.id)
            })
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.pills
            .iter()
            .map(|p| p.rect)
            .chain(self.groups.iter().map(|g| g.rect))
            .reduce(|a, b| a.union(b))
    }
}

/// One unpositioned item of a column: a lone pill or a group of pills.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Pill {
        id: SymbolId,
        label: String,
        kind: SymbolKind,
    },
    Group {
        id: Option<SymbolId>,
        title: String,
        kind: Option<SymbolKind>,
        members: Vec<(SymbolId, String, SymbolKind)>,
        hidden_members: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// `x` is the column's left edge.
    Left,
    /// `x` is the column's right edge.
    Right,
    /// `x` is the column's center.
    Center,
}

/// Measured size of a block.
pub fn block_size(block: &Block, measure: Measure) -> Vec2 {
    match block {
        Block::Pill { label, .. } => Vec2::new(pill_width(label, measure), PILL_HEIGHT),
        Block::Group {
            title,
            members,
            hidden_members,
            ..
        } => {
            let title_width = measure(title, TextRole::Title);
            let widest_member = members
                .iter()
                .map(|(_, label, _)| pill_width(label, measure))
                .fold(0.0, f32::max);
            let width = title_width.max(widest_member) + 2.0 * GROUP_PADDING;
            Vec2::new(width, group_height(members.len(), *hidden_members))
        }
    }
}

fn pill_width(label: &str, measure: Measure) -> f32 {
    (measure(label, TextRole::Pill) + 2.0 * PILL_PADDING_X).max(MIN_PILL_WIDTH)
}

fn group_height(member_count: usize, hidden_members: usize) -> f32 {
    let footer = if hidden_members > 0 {
        GROUP_FOOTER_HEIGHT
    } else {
        0.0
    };
    if member_count == 0 {
        return GROUP_TITLE_HEIGHT + footer;
    }
    let n = member_count as f32;
    GROUP_TITLE_HEIGHT + n * PILL_HEIGHT + (n - 1.0) * PILL_GAP + GROUP_PADDING + footer
}

/// Stacks `blocks` top-to-bottom, vertically centered on `center_y`, aligned
/// on `x` as `align` says. Returns the positioned groups and loose pills.
pub fn stack_column(
    blocks: Vec<Block>,
    x: f32,
    align: Align,
    center_y: f32,
    measure: Measure,
) -> (Vec<Group>, Vec<Pill>) {
    let sizes: Vec<Vec2> = blocks.iter().map(|b| block_size(b, measure)).collect();
    let total_height =
        sizes.iter().map(|s| s.y).sum::<f32>() + BLOCK_GAP * sizes.len().saturating_sub(1) as f32;
    let mut top = center_y - total_height / 2.0;
    let mut groups = Vec::new();
    let mut pills = Vec::new();

    for (block, size) in blocks.into_iter().zip(sizes) {
        let left = match align {
            Align::Left => x,
            Align::Right => x - size.x,
            Align::Center => x - size.x / 2.0,
        };
        let rect = Rect::from_min_size(Pos2::new(left, top), size);
        match block {
            Block::Pill { id, label, kind } => pills.push(Pill {
                id,
                label,
                kind,
                rect,
            }),
            Block::Group {
                id,
                title,
                kind,
                members,
                hidden_members,
            } => {
                groups.push(place_group(
                    rect,
                    id,
                    title,
                    kind,
                    members,
                    hidden_members,
                    measure,
                ));
            }
        }
        top += size.y + BLOCK_GAP;
    }
    (groups, pills)
}

fn place_group(
    rect: Rect,
    id: Option<SymbolId>,
    title: String,
    kind: Option<SymbolKind>,
    members: Vec<(SymbolId, String, SymbolKind)>,
    hidden_members: usize,
    measure: Measure,
) -> Group {
    let title_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), GROUP_TITLE_HEIGHT));
    let pills = members
        .into_iter()
        .enumerate()
        .map(|(i, (id, label, kind))| {
            let top = title_rect.bottom() + i as f32 * (PILL_HEIGHT + PILL_GAP);
            let min = Pos2::new(rect.left() + GROUP_PADDING, top);
            let size = Vec2::new(pill_width(&label, measure), PILL_HEIGHT);
            Pill {
                id,
                label,
                kind,
                rect: Rect::from_min_size(min, size),
            }
        })
        .collect();
    Group {
        id,
        title,
        kind,
        rect,
        title_rect,
        pills,
        hidden_members,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Deterministic stand-in for font metrics.
    pub fn fake_measure(text: &str, role: TextRole) -> f32 {
        let per_char = if role == TextRole::Title { 8.0 } else { 7.0 };
        text.chars().count() as f32 * per_char
    }

    pub fn assert_disjoint(rects: &[Rect]) {
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b), "overlap: {a:?} {b:?}");
            }
        }
    }

    fn pill_block(id: SymbolId, label: &str) -> Block {
        Block::Pill {
            id,
            label: label.into(),
            kind: SymbolKind::Function,
        }
    }

    fn group_block(id: SymbolId, members: &[(SymbolId, &str)]) -> Block {
        Block::Group {
            id: Some(id),
            title: format!("G{id}"),
            kind: Some(SymbolKind::Class),
            members: members
                .iter()
                .map(|(i, l)| (*i, l.to_string(), SymbolKind::Method))
                .collect(),
            hidden_members: 0,
        }
    }

    #[test]
    fn pill_size_fits_label_with_padding() {
        let size = block_size(&pill_block(1, "abcdefghij"), &fake_measure);
        assert_eq!(size, Vec2::new(70.0 + 2.0 * PILL_PADDING_X, PILL_HEIGHT));
        let tiny = block_size(&pill_block(1, "a"), &fake_measure);
        assert_eq!(tiny.x, MIN_PILL_WIDTH);
    }

    #[test]
    fn group_size_fits_title_and_members() {
        let size = block_size(
            &group_block(9, &[(1, "a"), (2, "a_long_member_name")]),
            &fake_measure,
        );
        let widest = 18.0 * 7.0 + 2.0 * PILL_PADDING_X;
        assert_eq!(size.x, widest + 2.0 * GROUP_PADDING);
        let expected_h = GROUP_TITLE_HEIGHT + 2.0 * PILL_HEIGHT + PILL_GAP + GROUP_PADDING;
        assert_eq!(size.y, expected_h);
    }

    #[test]
    fn stacked_blocks_do_not_overlap_and_respect_alignment() {
        let blocks = vec![
            pill_block(1, "one"),
            group_block(2, &[(3, "m")]),
            pill_block(4, "four_four"),
        ];
        let (groups, pills) = stack_column(blocks, 100.0, Align::Right, 0.0, &fake_measure);
        let mut rects: Vec<Rect> = pills.iter().map(|p| p.rect).collect();
        rects.extend(groups.iter().map(|g| g.rect));
        assert_disjoint(&rects);
        assert!(rects.iter().all(|r| (r.right() - 100.0).abs() < 1e-3));
        // Vertically centered around 0.
        let top = rects.iter().map(|r| r.top()).fold(f32::MAX, f32::min);
        let bottom = rects.iter().map(|r| r.bottom()).fold(f32::MIN, f32::max);
        assert!((top + bottom).abs() < 1e-3);
    }

    #[test]
    fn group_member_pills_sit_inside_their_box_below_the_title() {
        let (groups, _) = stack_column(
            vec![group_block(2, &[(3, "m"), (4, "n")])],
            0.0,
            Align::Left,
            0.0,
            &fake_measure,
        );
        let group = &groups[0];
        for pill in &group.pills {
            assert!(group.rect.contains_rect(pill.rect));
            assert!(pill.rect.top() >= group.title_rect.bottom());
        }
    }

    #[test]
    fn anchor_and_hit_find_pills_and_group_titles() {
        let blocks = vec![pill_block(1, "free"), group_block(2, &[(3, "member")])];
        let (groups, pills) = stack_column(blocks, 0.0, Align::Left, 0.0, &fake_measure);
        let scene = Scene {
            groups,
            pills,
            ..Scene::default()
        };

        let free = scene.anchor(1).expect("pill anchor");
        assert_eq!(scene.hit(free.center()), Some(1));
        let member = scene.anchor(3).expect("member anchor");
        assert_eq!(scene.hit(member.center()), Some(3));
        let title = scene.anchor(2).expect("group anchor");
        assert_eq!(scene.hit(title.center()), Some(2));
        assert_eq!(scene.hit(Pos2::new(-500.0, -500.0)), None);
        assert!(scene.anchor(99).is_none());
    }

    #[test]
    fn bounds_cover_everything_and_empty_scene_has_none() {
        assert!(Scene::default().bounds().is_none());
        let (groups, pills) = stack_column(
            vec![pill_block(1, "a"), group_block(2, &[(3, "b")])],
            0.0,
            Align::Center,
            0.0,
            &fake_measure,
        );
        let scene = Scene {
            groups,
            pills,
            ..Scene::default()
        };
        let bounds = scene.bounds().expect("bounds");
        assert!(scene.all_pills().all(|p| bounds.contains_rect(p.rect)));
        assert!(scene.groups.iter().all(|g| bounds.contains_rect(g.rect)));
    }
}
