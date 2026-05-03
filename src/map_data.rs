// ════════════════════════════════════════════════════════════════════════
//  World blueprint — 5 themed segments in a horizontal strip:
//      [Suburbs] → [Downtown] → [Industrial] → [Hospital] → [Military]
//  Total: 240×48 tiles (7680×1536 px).  Each segment is 48×48 tiles with
//  its own road grid (rows 22-25 horizontal, cols 22-25 vertical).
//
//  Buildings + props are baked from the deterministic generator in the
//  prototype HTML (`assets/zombiegame2-map.html`) via:
//      node /tmp/.../gen.js | node /tmp/.../emit_rust.js > world_consts.rs
//  See `src/world_consts.rs` for the actual data tables.  Consumed by
//  `map.rs::spawn_map`.
// ════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use bevy::math::IVec2;

// Re-export the baked tables so `map.rs` can use them directly.
// `ZOMBIE_SPAWNS` is currently unused — kept as future hook for pre-placed
// ambient zombies described in the spec (count = 3 + difficulty*2 per seg).
pub use crate::world_consts::{BUILDINGS, PROPS, SEGMENTS};
#[allow(unused_imports)]
pub use crate::world_consts::ZOMBIE_SPAWNS;

// ──── Segments ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Suburb,
    Downtown,
    Industrial,
    Hospital,
    Military,
}

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub id: u8,
    pub name: &'static str,
    pub pl_name: &'static str,
    pub difficulty: u8,
    pub theme: Theme,
}

// ──── Buildings ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingType {
    House,
    Shed,
    Garage,
    Shop,
    Apartment,
    Civic,
    Church,
    Market,
    Bank,
    Tower,
    Factory,
    Warehouse,
    Depot,
    Tank,
    Hospital,
    Morgue,
    Park,
    Bunker,
    Tent,
    Helipad,
    Gas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoofStyle {
    /// Pitched two-slope roof — houses, sheds, churches, market hall, park
    /// pavilion, gable tents.
    Gable,
    /// Flat parapet roof with AC unit highlight — most civic / commercial.
    Flat,
    /// Apartment-block grid of dark balcony rectangles + central elevator.
    Apt,
    /// Saw-tooth strips with skylights — factories.
    Saw,
    /// Cylindrical tank, ellipse fill.
    Round,
    /// Pyramidal canvas pitch — military tents.
    Tent,
    /// Helipad — flat concrete + circle + yellow "H".
    Pad,
}

#[derive(Clone, Copy, Debug)]
pub struct Building {
    pub seg_id: u8,
    pub kind: BuildingType,
    pub roof: RoofStyle,
    /// Local tile coords inside the segment (0..48).
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Door tile (local coords) — for visual marker only; buildings are
    /// solid blockers in this iteration (no enterable interiors yet).
    pub door: IVec2,
}

// ──── Props ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    // Suburb / hospital flora
    Tree,
    Bush,
    HedgeH,
    HedgeV,
    Planter,
    // Vehicles
    Car,
    Wreck,
    Bus,
    Truck,
    Ambulance,
    MilTruck,
    Jeep,
    // Urban scatter
    Mailbox,
    Trash,
    Lamp,
    Dumpster,
    Bench,
    Sign,
    Blood,
    Debris,
    // Industrial
    Container,
    Barrels,
    Pallet,
    Oil,
    Crane,
    Forklift,
    Crate,
    // Hospital
    Gurney,
    Playground,
    BodyBag,
    // Military
    SandbagH,
    SandbagV,
    RazorH,
    RazorV,
    Crater,
    Flag,
}

#[derive(Clone, Copy, Debug)]
pub struct Prop {
    pub seg_id: u8,
    pub kind: PropKind,
    /// Local tile coords inside the segment.
    pub x: i32,
    pub y: i32,
    /// Footprint in tiles (most are 1×1; hedges, vehicles, sandbags larger).
    pub w: i32,
    pub h: i32,
}

// ──── Gates between segments ────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub enum GateKind {
    /// Old river bridge: arched span over water.
    Bridge,
    /// Wall breach: rubble-walled passage.
    Breach,
    /// Underground service overpass / tunnel.
    Tunnel,
    /// Military checkpoint with barriers.
    Gate,
}

#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub from_seg: u8,
    pub to_seg: u8,
    pub kind: GateKind,
    /// Cost in dollars to unlock (= deduct from shared `Score`).
    pub cost: u32,
    pub label: &'static str,
}

/// Four gates connecting the 5 segments.  Player spends $$$ at each gate
/// to push deeper into the world — same model as before, applied to the
/// new layout.
pub const GATES: &[Gate] = &[
    Gate {
        from_seg: 1,
        to_seg: 2,
        kind: GateKind::Bridge,
        cost: 400,
        label: "OLD RIVER BRIDGE",
    },
    Gate {
        from_seg: 2,
        to_seg: 3,
        kind: GateKind::Breach,
        cost: 800,
        label: "WALL BREACH",
    },
    Gate {
        from_seg: 3,
        to_seg: 4,
        kind: GateKind::Bridge,
        cost: 1500,
        label: "SERVICE OVERPASS",
    },
    Gate {
        from_seg: 4,
        to_seg: 5,
        kind: GateKind::Gate,
        cost: 3000,
        label: "CHECKPOINT ALPHA",
    },
];

// ──── Compat shims kept so other modules keep compiling ─────────────────
// The legacy (rural-village) furniture and decor systems are no longer
// instantiated — buildings are solid blocks in this iteration and the
// prop system replaces street decor.  We keep the *types* here so older
// match arms don't break across the project.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingKind {
    House,
    Sheriff,
    Pharmacy,
    Diner,
    GeneralStore,
    Saloon,
    Church,
    GasStation,
}

#[derive(Clone, Copy, Debug)]
pub enum FurnitureKind {
    Desk,
    OfficeChair,
    FilingCabinet,
    WeaponsRack,
    JailBarsH,
    JailBarsV,
    Cot,
    Toilet,
    ShelfH,
    ShelfV,
    Counter,
    Register,
    Crate,
    Barrel,
    Freezer,
    BoothW,
    BoothE,
    DinerTable,
    DinerStool,
    Stove,
    Fridge,
    BarCounter,
    PoolTable,
    Piano,
    Pew,
    Altar,
    Cross,
    Candle,
    ToolBench,
    OilBarrel,
    CarLift,
    GasPump,
    Bed,
    Dresser,
    Nightstand,
    Couch,
    CoffeeTable,
    Tv,
    Sink,
    DiningTable,
    Rug,
    InternalWallH,
    InternalWallV,
}

#[derive(Clone, Copy)]
pub struct FurnitureSpec {
    pub kind: FurnitureKind,
    pub dx: f32,
    pub dy: f32,
}

pub const HOUSE_FURNITURE: &[FurnitureSpec] = &[];
pub const SHERIFF_FURNITURE: &[FurnitureSpec] = &[];
pub const PHARMACY_FURNITURE: &[FurnitureSpec] = &[];
pub const DINER_FURNITURE: &[FurnitureSpec] = &[];
pub const GENERAL_STORE_FURNITURE: &[FurnitureSpec] = &[];
pub const SALOON_FURNITURE: &[FurnitureSpec] = &[];
pub const CHURCH_FURNITURE: &[FurnitureSpec] = &[];
pub const GAS_STATION_FURNITURE: &[FurnitureSpec] = &[];

pub fn furniture_for(_kind: BuildingKind) -> &'static [FurnitureSpec] {
    &[]
}

#[derive(Clone, Copy, Debug)]
pub enum StreetDecorKind {
    PineTree,
    PineTreeSmall,
    Birch,
    DeadTree,
    DeadTree2,
    Bush,
    Stump,
    Rock,
    GrassPatch,
    Bus,
    CarWreck,
    CarWreckSedan,
    WoodFence,
    FencePost,
    Mailbox,
    Well,
    Firepit,
    LogPile,
    Rubble,
    BloodStain,
    Crack,
    SandbagPile,
    Barrel,
    Crate,
    LampPost,
    Bench,
    TrashCan,
    Dumpster,
    FireHydrant,
    StopSign,
    ShopSign,
    Tombstone,
    TombstoneCross,
    GasPump,
    CemeteryGate,
    NewspaperBox,
    PhoneBooth,
    BusStop,
    Billboard,
    RoadCone,
    BarbedWire,
}

#[derive(Clone, Copy)]
pub struct DecorSpec {
    pub kind: StreetDecorKind,
    pub x: f32,
    pub y: f32,
    pub rot: f32,
}

pub const STREET_DECOR: &[DecorSpec] = &[];
