#!/usr/bin/env python3
from pathlib import Path
import sys


TASKS = [
    (
        "Define core math and graph invariants",
        "Goal: Implement the core vector math and body graph invariants. Context: Read SPEC.md sections 1 and 2, then inspect crates/biolife_core/src/vec2.rs, body.rs, and tests/body_graph.rs. Requirements: provide Vec2 arithmetic helpers; support node and segment creation; preserve parent/child graph shape; expose segment kind/type data; reject invalid segment endpoints. Acceptance: from ./worktree, cargo test -p biolife_core body_graph passes; commit the code change in ./worktree; close only this task. Out of scope: growth, energy, combat, and fluid physics.",
    ),
    (
        "Implement chromosome developmental program",
        "Goal: Implement chromosome gene scheduling and deterministic sample chromosomes. Context: Read SPEC.md section 3, inspect chromosome.rs and tests/chromosome.rs. Requirements: represent genes with time windows, energy costs, parent nodes, segment kind, length, angle, and optional muscle torque; return only genes that are due and not yet expressed; keep output deterministic. Acceptance: from ./worktree, cargo test -p biolife_core chromosome passes; commit the code change; close only this task. Out of scope: mutating genomes or reproduction.",
    ),
    (
        "Implement growth engine",
        "Goal: Implement energy-gated organism growth from chromosome genes. Context: Depends on chromosome and body graph work. Read SPEC.md section 4, inspect organism.rs and tests/growth.rs. Requirements: advance developmental time, spend energy only when enough is available, attach new graph segments at requested parent nodes, and mark expressed genes exactly once. Acceptance: from ./worktree, cargo test -p biolife_core growth passes; commit the code change; close only this task. Out of scope: interactions and movement.",
    ),
    (
        "Implement energy harvesting and food intake",
        "Goal: Implement green solar harvesting and mouth food intake. Context: Read SPEC.md section 5, inspect world.rs and tests/energy.rs. Requirements: green segments harvest solar energy from world light; mouth segments consume nearby food; cap stored organism energy; remove consumed food deterministically. Acceptance: from ./worktree, cargo test -p biolife_core energy passes; commit the code change; close only this task. Out of scope: combat and locomotion.",
    ),
    (
        "Implement combat and defense interactions",
        "Goal: Implement red attack segments and shield defense. Context: Read SPEC.md section 6, inspect interactions.rs and tests/combat.rs. Requirements: red segment tips damage nearby organisms; shield segments reduce incoming damage; core health reaches zero only after mitigated damage; do not apply self-damage. Acceptance: from ./worktree, cargo test -p biolife_core combat passes; commit the code change; close only this task. Out of scope: reproduction and advanced collision detection.",
    ),
    (
        "Implement propulsion intent and muscle torque",
        "Goal: Convert muscle segments and chromosome torque commands into joint propulsion intents. Context: Read SPEC.md section 7, inspect physics.rs and tests/propulsion.rs. Requirements: muscle segments can carry signed torque; torque at a node produces equal/opposite perpendicular force intents on connected parent/child sides; force magnitude is deterministic and bounded. Acceptance: from ./worktree, cargo test -p biolife_core propulsion passes; commit the code change; close only this task. Out of scope: high-fidelity Navier-Stokes or inertial rigid body simulation.",
    ),
    (
        "Implement viscous fluid integration",
        "Goal: Implement a simple overdamped viscous-fluid movement model. Context: Builds on propulsion intents. Read SPEC.md section 8, inspect physics.rs and tests/fluid.rs. Requirements: integrate node positions with velocity = force / drag, enforce segment rest lengths with iterative constraint projection, apply angular damping, and keep center-of-mass drift plausible without explosive energy. Acceptance: from ./worktree, cargo test -p biolife_core fluid passes; commit the code change; close only this task. Out of scope: exact continuum fluid dynamics.",
    ),
    (
        "Implement deterministic world tick",
        "Goal: Implement deterministic simulation stepping across growth, energy, interactions, and physics. Context: Read SPEC.md section 9, inspect world.rs and tests/world_tick.rs. Requirements: tick order is grow, harvest/eat, combat, propulsion, integrate; results are deterministic for a fixed scenario; dead organisms stop acting. Acceptance: from ./worktree, cargo test -p biolife_core world_tick passes; commit the code change; close only this task. Out of scope: rendering.",
    ),
    (
        "Expose offline simulation API",
        "Goal: Expose a clean backend API for offline simulation and snapshots. Context: Read SPEC.md section 10, inspect lib.rs and tests/offline_api.rs. Requirements: public core API can create scenarios, run N ticks, and return serializable-ish snapshots without depending on the app crate or stdout. Acceptance: from ./worktree, cargo test -p biolife_core offline_api passes; commit the code change; close only this task. Out of scope: UI rendering or networking.",
    ),
    (
        "Wire native graphical Rust frontend",
        "Goal: Implement biolife_app as a native graphical Rust app over biolife_core. Context: Read SPEC.md section 9, inspect crates/biolife_app/src/main.rs, Cargo.toml, and tests/cli.rs. Requirements: support `biolife_app --ticks N --light L --drag D` as a windowed Rust app by default, provide `--headless` for CI-safe text summaries, run simulation state through core APIs, render organisms moving and evolving in real time in a native window, expose play/pause/reset/step/speed and parameter controls, update organism/segment inspectors while the app runs, and keep simulation rules out of the app crate. Acceptance: from ./worktree, cargo test --workspace passes; commit the code change; close this task and close the parent epic if all children are closed. Out of scope: HTML/SVG/browser export, webviews, external services, or moving simulation rules into the app crate.",
    ),
]


def main() -> None:
    root = Path(sys.argv[1])
    worktree = root / "worktree"
    write_bookkeeping(root)
    write_worktree(worktree)


def write_bookkeeping(root: Path) -> None:
    (root / ".mrmouth").mkdir(parents=True, exist_ok=True)
    (root / ".gitignore").write_text(
        ".codex-home/\n.eval-epic-id\n.eval-leaf-ids\n.goal-objective.txt\n.goal-turn.txt\nworktree/\ntmp/\nlogs/\n",
        encoding="utf-8",
    )
    (root / ".mrmouth" / "config.toml").write_text(
        'agent = "codex"\n\n[do]\ntimeout = 90\nmax_failures = 1\n',
        encoding="utf-8",
    )
    (root / ".mrmouth" / "prompt.md").write_text(
        "You are running inside a deterministic Biolife eval fixture.\n\n"
        "Complete the claimed Litebrite work exactly as described. The application code lives in ./worktree. "
        "Make focused code commits in ./worktree, close completed Litebrite items in this bookkeeping repo, "
        "and do not edit tests unless the task explicitly asks for it.\n",
        encoding="utf-8",
    )
    (root / "SPEC.md").write_text(spec_text(), encoding="utf-8")
    (root / ".eval-task-list.tsv").write_text(
        "\n".join(f"{index}\t{title}\t{description}" for index, (title, description) in enumerate(TASKS, 1)) + "\n",
        encoding="utf-8",
    )


def write_worktree(worktree: Path) -> None:
    core = worktree / "crates" / "biolife_core"
    app = worktree / "crates" / "biolife_app"
    (core / "src").mkdir(parents=True, exist_ok=True)
    (core / "tests").mkdir(parents=True, exist_ok=True)
    (app / "src").mkdir(parents=True, exist_ok=True)
    (app / "tests").mkdir(parents=True, exist_ok=True)
    (worktree / ".gitignore").write_text("target/\n", encoding="utf-8")
    (worktree / "Cargo.toml").write_text(workspace_toml(), encoding="utf-8")
    (worktree / "check.sh").write_text("#!/usr/bin/env bash\nset -euo pipefail\ncargo test --workspace\n", encoding="utf-8")
    (worktree / "check.sh").chmod(0o755)
    (core / "Cargo.toml").write_text(core_toml(), encoding="utf-8")
    (app / "Cargo.toml").write_text(app_toml(), encoding="utf-8")

    for name, content in core_sources().items():
        (core / "src" / name).write_text(content, encoding="utf-8")
    for name, content in core_tests().items():
        (core / "tests" / name).write_text(content, encoding="utf-8")
    (app / "src" / "main.rs").write_text(app_main(), encoding="utf-8")
    (app / "tests" / "cli.rs").write_text(app_cli_test(), encoding="utf-8")


def spec_text() -> str:
    return """# Biolife Eval Spec

Biolife is a small Rust simulation/game prototype. Little critters are encoded
by a chromosome. A chromosome governs how an organism grows over developmental
time when enough energy is available.

The important design constraint is architectural: simulation logic belongs in
`biolife_core` and must be usable offline without the frontend. `biolife_app`
is a native graphical Rust app over the core API. It may expose a headless CLI
mode for tests and offline automation, but its main responsibility is a windowed
desktop app where organisms visibly move and evolve over time, parameters can be
changed, and organisms/segments can be inspected without moving simulation rules
out of the core crate.

## 1. Core Math And Graph Body

Creatures are graphs of nodes and segments. A segment connects two nodes and has
a `SegmentKind`: `Core`, `Green`, `Red`, `Mouth`, `Shield`, or `Muscle`.
Segments have rest length and optional torque. Node positions are 2D.

The body graph must preserve stable node/segment ids and reject invalid segment
endpoints. It is acceptable for the initial eval to use tree-shaped organisms,
but the representation should not hard-code a single chain.

## 2. Segment Semantics

`Green` harvests solar energy. `Red` hurts nearby organisms. `Mouth` consumes
food particles. `Shield` mitigates incoming damage. `Muscle` is a propulsion
segment that creates torque around a node. `Core` represents the root/life
segment.

## 3. Chromosome

A chromosome is a deterministic list of genes. Each gene has:

- expression time window
- energy cost
- parent node id
- segment kind
- length
- relative angle in radians
- optional signed torque for muscle segments

When developmental time reaches a gene and the organism has enough energy, the
gene expresses exactly once and attaches a new segment/node to the body graph.

## 4. Growth And Energy

Organisms start with a core segment, initial energy, max energy, and health.
Growth spends energy. Lack of energy delays expression but does not skip the
gene forever.

Green segments harvest `light * length * 0.5` energy per tick. Mouth segments
eat food within a small radius from their tip; food is removed once eaten.

## 5. Combat And Defense

Red segment tips deal damage to nearby organisms. Shield segments reduce
incoming damage by a bounded amount. Red segments must not damage their own
organism. Dead organisms stop acting.

## 6. Propulsion And Viscous Fluid

Do not attempt full Navier-Stokes. Use a tiny overdamped viscous model:

- collect forces from active muscle torque commands
- velocity is `force / drag`
- integrate node positions by `dt`
- iteratively project segment endpoints back toward rest length
- apply angular damping so torque does not explode

Torque at a joint should create movement by applying equal/opposite
perpendicular force intents to the connected segment sides. This is a deliberate
approximation: the behavior must be deterministic, stable, and plausible in a
viscous fluid, not physically perfect.

## 7. World Tick

The deterministic tick order is:

1. growth
2. solar harvesting and food eating
3. combat/defense
4. propulsion force generation
5. viscous integration

## 8. Offline API

`biolife_core` must expose a public API to create a sample scenario, run a fixed
number of ticks, and return a snapshot containing organism count, alive count,
energy, health, segment counts, and positions. This API is what offline
analysis, tests, and future renderers should use.

## 9. Frontend Boundary And Native Graphical App

`biolife_app` should parse `--ticks N`, `--light L`, `--drag D`, and optional
`--headless`. Without `--headless`, it should launch a native graphical Rust
window. With `--headless`, it should run through `biolife_core` and print a
stable text summary for CI and offline automation.

The graphical app must be built as Rust UI/rendering code, for example with
`eframe`/`egui`, `macroquad`, `winit`/`wgpu`, `bevy`, or an equivalent native
Rust windowing/rendering stack. It must include:

- a native application window, not a browser, generated HTML file, webview, or SVG export
- a real-time update/render loop that advances the simulated world while running
- parameter controls for ticks, light, drag, playback speed, and paused/running state
- play/pause, reset, step, and scrub controls
- a graphical visualization of organisms, nodes, segments, food, and motion over time
- color-coded segment rendering for core, green, red, mouth, shield, and muscle
- an organism inspector that updates with energy, health, segment count, and node positions
- a segment inspector that updates with segment id, kind, endpoints, length, and torque

A static final-frame export is not enough for this eval. HTML/SVG/browser output
is not enough for this eval. The Rust app must make the creatures move and
evolve in front of the user in a native graphical window. The app crate may own
input handling, rendering, and UI state, but it must not reimplement growth,
combat, energy, or physics rules; all simulated state must come from
`biolife_core`.
"""


def workspace_toml() -> str:
    return """[workspace]
members = [
    "crates/biolife_core",
    "crates/biolife_app",
]
resolver = "2"
"""


def core_toml() -> str:
    return """[package]
name = "biolife_core"
version = "0.1.0"
edition = "2021"
"""


def app_toml() -> str:
    return """[package]
name = "biolife_app"
version = "0.1.0"
edition = "2021"

[dependencies]
biolife_core = { path = "../biolife_core" }
"""


def core_sources() -> dict[str, str]:
    return {
        "lib.rs": """pub mod body;
pub mod chromosome;
pub mod interactions;
pub mod organism;
pub mod physics;
pub mod vec2;
pub mod world;

pub use body::{Body, NodeId, SegmentId, SegmentKind};
pub use chromosome::{Chromosome, Gene};
pub use organism::{Organism, OrganismId};
pub use world::{run_sample, Food, Snapshot, World};
""",
        "vec2.rs": """use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        todo!("compute vector length")
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<f32> for Vec2 {
    type Output = Self;

    fn div(self, rhs: f32) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs)
    }
}
""",
        "body.rs": """use crate::vec2::Vec2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SegmentId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentKind {
    Core,
    Green,
    Red,
    Mouth,
    Shield,
    Muscle,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub position: Vec2,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub id: SegmentId,
    pub a: NodeId,
    pub b: NodeId,
    pub kind: SegmentKind,
    pub rest_length: f32,
    pub torque: f32,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub nodes: Vec<Node>,
    pub segments: Vec<Segment>,
}

impl Body {
    pub fn new_core() -> Self {
        todo!("create a core body")
    }

    pub fn add_segment(
        &mut self,
        parent: NodeId,
        kind: SegmentKind,
        length: f32,
        angle: f32,
        torque: f32,
    ) -> Result<(NodeId, SegmentId), String> {
        let _ = (parent, kind, length, angle, torque);
        todo!("attach a segment to the body graph")
    }

    pub fn node_position(&self, id: NodeId) -> Option<Vec2> {
        self.nodes.iter().find(|node| node.id == id).map(|node| node.position)
    }
}
""",
        "chromosome.rs": """use crate::body::{NodeId, SegmentKind};

#[derive(Clone, Debug)]
pub struct Gene {
    pub id: &'static str,
    pub start_time: f32,
    pub end_time: f32,
    pub energy_cost: f32,
    pub parent: NodeId,
    pub kind: SegmentKind,
    pub length: f32,
    pub angle: f32,
    pub torque: f32,
}

#[derive(Clone, Debug)]
pub struct Chromosome {
    pub genes: Vec<Gene>,
}

impl Chromosome {
    pub fn sample_swimmer() -> Self {
        todo!("return a deterministic sample swimmer chromosome")
    }

    pub fn due_genes<'a>(&'a self, time: f32, expressed: &[&str]) -> Vec<&'a Gene> {
        let _ = (time, expressed);
        todo!("return due unexpressed genes in deterministic order")
    }
}
""",
        "organism.rs": """use crate::body::Body;
use crate::chromosome::Chromosome;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct OrganismId(pub usize);

#[derive(Clone, Debug)]
pub struct Organism {
    pub id: OrganismId,
    pub body: Body,
    pub chromosome: Chromosome,
    pub expressed_genes: Vec<&'static str>,
    pub developmental_time: f32,
    pub energy: f32,
    pub max_energy: f32,
    pub health: f32,
}

impl Organism {
    pub fn sample(id: OrganismId) -> Self {
        todo!("create a sample organism")
    }

    pub fn grow(&mut self, dt: f32) {
        let _ = dt;
        todo!("express affordable due genes")
    }

    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }
}
""",
        "interactions.rs": """use crate::organism::Organism;
use crate::world::Food;

pub fn harvest_and_eat(organism: &mut Organism, food: &mut Vec<Food>, light: f32) {
    let _ = (organism, food, light);
    todo!("harvest solar energy and consume nearby food")
}

pub fn apply_combat(organisms: &mut [Organism]) {
    let _ = organisms;
    todo!("apply red segment damage with shield mitigation")
}
""",
        "physics.rs": """use crate::organism::Organism;

#[derive(Clone, Copy, Debug)]
pub struct PhysicsParams {
    pub drag: f32,
    pub dt: f32,
    pub constraint_iterations: usize,
}

impl Default for PhysicsParams {
    fn default() -> Self {
        Self {
            drag: 8.0,
            dt: 0.1,
            constraint_iterations: 4,
        }
    }
}

pub fn apply_propulsion(organism: &mut Organism, params: PhysicsParams) {
    let _ = (organism, params);
    todo!("apply muscle torque and integrate viscous movement")
}
""",
        "world.rs": """use crate::interactions::{apply_combat, harvest_and_eat};
use crate::organism::{Organism, OrganismId};
use crate::physics::{apply_propulsion, PhysicsParams};
use crate::vec2::Vec2;

#[derive(Clone, Debug)]
pub struct Food {
    pub position: Vec2,
    pub energy: f32,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tick: u64,
    pub organism_count: usize,
    pub alive_count: usize,
    pub total_segments: usize,
    pub first_energy: f32,
    pub first_health: f32,
    pub first_position: Vec2,
}

#[derive(Clone, Debug)]
pub struct World {
    pub tick: u64,
    pub light: f32,
    pub food: Vec<Food>,
    pub organisms: Vec<Organism>,
    pub physics: PhysicsParams,
}

impl World {
    pub fn sample() -> Self {
        todo!("create a deterministic sample world")
    }

    pub fn step(&mut self) {
        for organism in &mut self.organisms {
            organism.grow(self.physics.dt);
            harvest_and_eat(organism, &mut self.food, self.light);
        }
        apply_combat(&mut self.organisms);
        for organism in &mut self.organisms {
            if organism.is_alive() {
                apply_propulsion(organism, self.physics);
            }
        }
        self.tick += 1;
    }

    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let first = self.organisms.first();
        Snapshot {
            tick: self.tick,
            organism_count: self.organisms.len(),
            alive_count: self.organisms.iter().filter(|organism| organism.is_alive()).count(),
            total_segments: self.organisms.iter().map(|organism| organism.body.segments.len()).sum(),
            first_energy: first.map(|organism| organism.energy).unwrap_or(0.0),
            first_health: first.map(|organism| organism.health).unwrap_or(0.0),
            first_position: first
                .and_then(|organism| organism.body.node_position(crate::body::NodeId(0)))
                .unwrap_or(Vec2::ZERO),
        }
    }
}

pub fn run_sample(ticks: u64) -> Snapshot {
    let mut world = World::sample();
    world.run(ticks);
    world.snapshot()
}
""",
    }


def core_tests() -> dict[str, str]:
    return {
        "body_graph.rs": """use biolife_core::{Body, NodeId, SegmentKind};

#[test]
fn body_graph_preserves_segment_ids_and_positions() {
    let mut body = Body::new_core();
    let (green_node, green_segment) = body.add_segment(NodeId(0), SegmentKind::Green, 2.0, 0.0, 0.0).unwrap();
    let (_, red_segment) = body.add_segment(green_node, SegmentKind::Red, 1.5, 1.5707964, 0.0).unwrap();
    assert_eq!(green_node.0, 1);
    assert_eq!(green_segment.0, 1);
    assert_eq!(red_segment.0, 2);
    assert_eq!(body.nodes.len(), 3);
    assert_eq!(body.segments.len(), 3);
    assert!(body.add_segment(NodeId(99), SegmentKind::Mouth, 1.0, 0.0, 0.0).is_err());
}
""",
        "chromosome.rs": """use biolife_core::Chromosome;

#[test]
fn chromosome_returns_due_unexpressed_genes() {
    let chromosome = Chromosome::sample_swimmer();
    let due = chromosome.due_genes(2.0, &[]);
    assert_eq!(due.iter().map(|gene| gene.id).collect::<Vec<_>>(), vec!["leaf-left", "leaf-right", "tail-muscle"]);
    let due = chromosome.due_genes(2.0, &["leaf-left"]);
    assert_eq!(due.iter().map(|gene| gene.id).collect::<Vec<_>>(), vec!["leaf-right", "tail-muscle"]);
}
""",
        "growth.rs": """use biolife_core::{Organism, OrganismId, SegmentKind};

#[test]
fn growth_spends_energy_and_expresses_once() {
    let mut organism = Organism::sample(OrganismId(0));
    organism.energy = 20.0;
    organism.grow(2.0);
    assert!(organism.body.segments.iter().any(|segment| segment.kind == SegmentKind::Green));
    let count = organism.body.segments.len();
    let energy_after = organism.energy;
    organism.grow(0.5);
    assert_eq!(organism.body.segments.len(), count);
    assert_eq!(organism.energy, energy_after);
}
""",
        "energy.rs": """use biolife_core::{Organism, OrganismId, SegmentKind, Food};
use biolife_core::interactions::harvest_and_eat;
use biolife_core::vec2::Vec2;

#[test]
fn green_segments_harvest_and_mouths_consume_food() {
    let mut organism = Organism::sample(OrganismId(0));
    organism.energy = 20.0;
    organism.grow(10.0);
    let before = organism.energy;
    let mut food = vec![Food { position: Vec2::new(1.0, 0.0), energy: 4.0 }];
    harvest_and_eat(&mut organism, &mut food, 3.0);
    assert!(organism.energy > before);
    assert!(organism.energy <= organism.max_energy);
    assert!(organism.body.segments.iter().any(|segment| segment.kind == SegmentKind::Mouth));
    assert!(food.len() < 1);
}
""",
        "combat.rs": """use biolife_core::{Organism, OrganismId};
use biolife_core::interactions::apply_combat;

#[test]
fn red_damage_is_mitigated_by_shields_and_not_self_applied() {
    let mut attacker = Organism::sample(OrganismId(0));
    let mut defender = Organism::sample(OrganismId(1));
    attacker.energy = 50.0;
    defender.energy = 50.0;
    attacker.grow(10.0);
    defender.grow(10.0);
    let attacker_health = attacker.health;
    let defender_health = defender.health;
    let mut organisms = vec![attacker, defender];
    apply_combat(&mut organisms);
    assert_eq!(organisms[0].health, attacker_health);
    assert!(organisms[1].health < defender_health);
    assert!(organisms[1].health > defender_health - 10.0);
}
""",
        "propulsion.rs": """use biolife_core::{Organism, OrganismId};
use biolife_core::physics::{apply_propulsion, PhysicsParams};

#[test]
fn muscle_torque_moves_connected_nodes_in_opposite_lateral_directions() {
    let mut organism = Organism::sample(OrganismId(0));
    organism.energy = 50.0;
    organism.grow(3.0);
    let before_root = organism.body.nodes[0].position;
    let before_tail = organism.body.nodes.last().unwrap().position;
    apply_propulsion(&mut organism, PhysicsParams::default());
    let after_root = organism.body.nodes[0].position;
    let after_tail = organism.body.nodes.last().unwrap().position;
    assert_ne!(after_tail, before_tail);
    assert_ne!(after_root, before_root);
    assert!((after_tail.y - before_tail.y).signum() != (after_root.y - before_root.y).signum());
}
""",
        "fluid.rs": """use biolife_core::{Organism, OrganismId};
use biolife_core::physics::{apply_propulsion, PhysicsParams};

#[test]
fn viscous_integration_preserves_rest_lengths_approximately() {
    let mut organism = Organism::sample(OrganismId(0));
    organism.energy = 50.0;
    organism.grow(10.0);
    for _ in 0..20 {
        apply_propulsion(&mut organism, PhysicsParams::default());
    }
    for segment in &organism.body.segments {
        let a = organism.body.node_position(segment.a).unwrap();
        let b = organism.body.node_position(segment.b).unwrap();
        let length = (b - a).length();
        assert!((length - segment.rest_length).abs() < 0.2, "{length} vs {}", segment.rest_length);
    }
}
""",
        "world_tick.rs": """use biolife_core::World;

#[test]
fn world_tick_is_deterministic_and_orders_systems() {
    let mut a = World::sample();
    let mut b = World::sample();
    a.run(12);
    b.run(12);
    let sa = a.snapshot();
    let sb = b.snapshot();
    assert_eq!(sa.tick, 12);
    assert_eq!(sa.total_segments, sb.total_segments);
    assert_eq!(sa.first_energy, sb.first_energy);
    assert_eq!(sa.first_position, sb.first_position);
    assert!(sa.total_segments > 4);
}
""",
        "offline_api.rs": """use biolife_core::{run_sample, World};

#[test]
fn core_exposes_offline_simulation_snapshots() {
    let mut world = World::sample();
    world.run(8);
    let snapshot = world.snapshot();
    assert_eq!(snapshot.tick, 8);
    assert_eq!(snapshot.organism_count, 2);
    assert!(snapshot.alive_count >= 1);
    assert!(snapshot.total_segments >= 6);

    let snapshot2 = run_sample(8);
    assert_eq!(snapshot2.tick, snapshot.tick);
    assert_eq!(snapshot2.total_segments, snapshot.total_segments);
}
""",
    }


def app_main() -> str:
    return """fn main() {
    todo!("parse parameters, run core simulation, and launch native graphical app")
}
"""


def app_cli_test() -> str:
    return """use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn headless_cli_runs_sample_simulation_through_core() {
    let exe = env!("CARGO_BIN_EXE_biolife_app");
    let output = Command::new(exe)
        .args(["--headless", "--ticks", "8", "--light", "2.5", "--drag", "6.0"])
        .output()
        .expect("run biolife_app");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("biolife"));
    assert!(stdout.contains("tick=8"));
    assert!(stdout.contains("organisms=2"));
    assert!(stdout.contains("segments="));
    assert!(stdout.contains("light=2.50"));
    assert!(stdout.contains("drag=6.00"));
}

#[test]
fn app_is_native_graphical_rust_frontend() {
    let app_dir = app_dir();
    let cargo = fs::read_to_string(app_dir.join("Cargo.toml")).expect("read app Cargo.toml");
    let main = fs::read_to_string(app_dir.join("src/main.rs")).expect("read app main");
    let combined = format!("{}\\n{}", cargo, main);
    let lower = combined.to_lowercase();

    assert!(
        lower.contains("eframe")
            || lower.contains("egui")
            || lower.contains("macroquad")
            || lower.contains("winit")
            || lower.contains("wgpu")
            || lower.contains("bevy"),
        "biolife_app must use a native Rust GUI/windowing/rendering crate"
    );
    assert!(
        lower.contains("run_native")
            || lower.contains("eventloop")
            || lower.contains("macroquad::main")
            || lower.contains("impl eframe::app")
            || lower.contains("impl app for")
            || lower.contains("bevy::prelude"),
        "biolife_app must launch a native Rust app/window"
    );
    assert!(combined.contains("biolife_core"));
    assert!(lower.contains("headless"));
    assert!(lower.contains("play"));
    assert!(lower.contains("pause"));
    assert!(lower.contains("reset"));
    assert!(lower.contains("step"));
    assert!(lower.contains("speed"));
    assert!(lower.contains("scrub"));
    assert!(lower.contains("organism"));
    assert!(lower.contains("segment"));
    assert!(lower.contains("inspector"));
    assert!(!lower.contains("<html"));
    assert!(!lower.contains("requestanimationframe"));
    assert!(!lower.contains("window.biolife_snapshots"));
    assert!(!lower.contains("--gui"));
}

#[test]
fn cli_reports_helpfully_for_help_flag() {
    let exe = env!("CARGO_BIN_EXE_biolife_app");
    let output = Command::new(exe)
        .arg("--help")
        .output()
        .expect("run biolife_app --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--ticks"));
    assert!(stdout.contains("--light"));
    assert!(stdout.contains("--drag"));
    assert!(stdout.contains("--headless"));
    assert!(!stdout.contains("--gui"));
}
"""


if __name__ == "__main__":
    main()
