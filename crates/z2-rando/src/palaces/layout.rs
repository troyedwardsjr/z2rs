//! A palace as a graph of placed rooms, and the checks every generated
//! palace must pass.

use std::collections::VecDeque;

use super::rooms::{Role, Room, RoomKey};

/// Index of a room inside a [`Layout`].
pub type Ix = usize;

/// The ways out of a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dir {
    /// Walking off the left edge.
    Left,
    /// Walking off the right edge.
    Right,
    /// Riding the elevator up.
    Up,
    /// Riding the elevator down.
    Down,
    /// Falling through the hole in the floor.
    Drop,
}

impl Dir {
    /// All directions.
    pub const ALL: [Dir; 5] = [Dir::Left, Dir::Right, Dir::Up, Dir::Down, Dir::Drop];

    /// The direction that leads back (a drop has none).
    #[must_use]
    pub fn opposite(self) -> Option<Dir> {
        match self {
            Dir::Left => Some(Dir::Right),
            Dir::Right => Some(Dir::Left),
            Dir::Up => Some(Dir::Down),
            Dir::Down => Some(Dir::Up),
            Dir::Drop => None,
        }
    }
}

/// One room in a palace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// The room (content, shape, vanilla bytes).
    pub room: Room,
    /// Map number, once assigned.
    pub map: Option<u8>,
    /// Neighbour per direction.
    pub left: Option<Ix>,
    /// See [`Placed::left`].
    pub right: Option<Ix>,
    /// See [`Placed::left`].
    pub up: Option<Ix>,
    /// See [`Placed::left`].
    pub down: Option<Ix>,
    /// See [`Placed::left`].
    pub drop: Option<Ix>,
    /// Replacement sideview bytes (a changed copy of the room's layout).
    pub sideview: Option<Vec<u8>>,
    /// Grid position used by coordinate-based generators (for the
    /// spoiler map only).
    pub pos: Option<(i32, i32)>,
}

impl Placed {
    /// A new, unconnected placement of `room`.
    #[must_use]
    pub fn new(room: Room) -> Placed {
        Placed {
            room,
            map: None,
            left: None,
            right: None,
            up: None,
            down: None,
            drop: None,
            sideview: None,
            pos: None,
        }
    }

    /// Neighbour in `d`.
    #[must_use]
    pub fn get(&self, d: Dir) -> Option<Ix> {
        match d {
            Dir::Left => self.left,
            Dir::Right => self.right,
            Dir::Up => self.up,
            Dir::Down => self.down,
            Dir::Drop => self.drop,
        }
    }

    /// Set the neighbour in `d`.
    pub fn set(&mut self, d: Dir, v: Option<Ix>) {
        match d {
            Dir::Left => self.left = v,
            Dir::Right => self.right = v,
            Dir::Up => self.up = v,
            Dir::Down => self.down = v,
            Dir::Drop => self.drop = v,
        }
    }

    /// The room offers an exit in `d`.
    #[must_use]
    pub fn has_exit(&self, d: Dir) -> bool {
        let s = &self.room.shape;
        match d {
            Dir::Left => s.left,
            Dir::Right => s.right,
            Dir::Up => s.up,
            Dir::Down => s.down,
            Dir::Drop => s.drop.is_some(),
        }
    }

    /// Neighbours in direction order.
    pub fn neighbours(&self) -> impl Iterator<Item = (Dir, Ix)> + '_ {
        Dir::ALL
            .into_iter()
            .filter_map(move |d| self.get(d).map(|n| (d, n)))
    }
}

/// A whole palace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Palace number (1-7).
    pub palace: u8,
    /// Rooms; index 0 is the entrance.
    pub rooms: Vec<Placed>,
    /// Name of the generator that built it.
    pub style: &'static str,
    /// The boss room continues to more palace through its right side.
    pub boss_continues: bool,
}

impl Layout {
    /// Index of the first room with `role`.
    #[must_use]
    pub fn find(&self, role: Role) -> Option<Ix> {
        self.rooms.iter().position(|p| p.room.role == role)
    }

    /// Indices of every room with `role`.
    #[must_use]
    pub fn all(&self, role: Role) -> Vec<Ix> {
        (0..self.rooms.len())
            .filter(|&i| self.rooms[i].room.role == role)
            .collect()
    }

    /// Entrance index.
    #[must_use]
    pub fn entrance(&self) -> Ix {
        self.find(Role::Entrance).unwrap_or(0)
    }

    /// Boss index.
    #[must_use]
    pub fn boss(&self) -> Option<Ix> {
        self.find(Role::Boss)
    }

    /// Two-way link `a --d--> b` (and `b --opposite--> a`). Drops are
    /// one-way by nature.
    pub fn link(&mut self, a: Ix, d: Dir, b: Ix) {
        self.rooms[a].set(d, Some(b));
        if let Some(o) = d.opposite() {
            self.rooms[b].set(o, Some(a));
        }
    }

    /// Rooms reachable from `from` without entering `blocked`, following
    /// every exit (with backtracking: Link can always walk back the way he
    /// came, which the two-way links already encode).
    #[must_use]
    pub fn reachable_from(&self, from: Ix, blocked: Option<Ix>) -> Vec<bool> {
        let mut seen = vec![false; self.rooms.len()];
        if Some(from) == blocked {
            return seen;
        }
        let mut q = VecDeque::from([from]);
        seen[from] = true;
        while let Some(i) = q.pop_front() {
            for (_, n) in self.rooms[i].neighbours() {
                if n < seen.len() && !seen[n] && Some(n) != blocked {
                    seen[n] = true;
                    q.push_back(n);
                }
            }
        }
        seen
    }

    /// Steps from the entrance to each room (`usize::MAX` = unreachable).
    #[must_use]
    pub fn distances(&self) -> Vec<usize> {
        let mut d = vec![usize::MAX; self.rooms.len()];
        let s = self.entrance();
        d[s] = 0;
        let mut q = VecDeque::from([s]);
        while let Some(i) = q.pop_front() {
            for (_, n) in self.rooms[i].neighbours() {
                if d[n] == usize::MAX {
                    d[n] = d[i] + 1;
                    q.push_back(n);
                }
            }
        }
        d
    }

    /// Every exit the room shape offers is connected (except the
    /// entrance's way out on the left and the boss room's way out on the
    /// right when the boss does not continue).
    #[must_use]
    pub fn open_exits(&self) -> Vec<(Ix, Dir)> {
        let mut v = Vec::new();
        for (i, p) in self.rooms.iter().enumerate() {
            for d in Dir::ALL {
                if !p.has_exit(d) || p.get(d).is_some() {
                    continue;
                }
                if p.room.role == Role::Entrance && d == Dir::Left {
                    continue;
                }
                if p.room.role == Role::Boss && d == Dir::Right && !self.boss_continues {
                    continue;
                }
                v.push((i, d));
            }
        }
        v
    }

    /// Links that use an exit the room does not have, or a drop into a room
    /// that cannot be landed in.
    #[must_use]
    pub fn bad_links(&self) -> Vec<(Ix, Dir)> {
        let mut v = Vec::new();
        for (i, p) in self.rooms.iter().enumerate() {
            for (d, n) in p.neighbours() {
                if !p.has_exit(d) {
                    v.push((i, d));
                    continue;
                }
                let t = &self.rooms[n];
                let ok = match d {
                    Dir::Left => t.room.shape.right,
                    Dir::Right => t.room.shape.left,
                    Dir::Up => t.room.shape.down || t.room.elevator.is_some(),
                    Dir::Down => t.room.shape.up || t.room.elevator.is_some(),
                    Dir::Drop => t.room.shape.zone.is_some(),
                };
                if !ok {
                    v.push((i, d));
                }
            }
        }
        v
    }

    /// Vanilla room keys in use (for duplicate checks and the spoiler).
    #[must_use]
    pub fn keys(&self) -> Vec<RoomKey> {
        self.rooms.iter().map(|p| p.room.key).collect()
    }
}

/// Where a palace drop must lead (see
/// [`crate::flags::PalaceDropStyle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropRule {
    /// The area a drop lands in must lead back to the entrance.
    Entrance,
    /// ... to the entrance or to a boss room.
    EntranceOrBoss,
    /// No check.
    Anything,
}

/// Checks a generated palace must pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// The Thunderbird room must stand between the entrance and Dark Link.
    pub require_thunderbird: bool,
    /// Fewest rooms from the entrance to the boss (0 = no check).
    pub boss_min_distance: usize,
    /// Drop safety.
    pub drops: DropRule,
}

/// Why a layout was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// An exit is left unconnected.
    OpenExit(Ix, Dir),
    /// A link uses an exit that is not there.
    BadLink(Ix, Dir),
    /// Not every room can be reached.
    Unreachable(Ix),
    /// The boss can be reached without passing Thunderbird.
    ThunderbirdBypass,
    /// The boss is too close.
    BossTooClose(usize),
    /// The boss room can be walked into from its right side before the
    /// boss is beaten.
    BossFromRight,
    /// A drop leads to a dead area.
    DropTrap(Ix),
    /// Missing the entrance or the boss.
    MissingRole(&'static str),
}

/// Validate `l` against `rules`.
pub fn validate(l: &Layout, rules: &Rules) -> Result<(), Invalid> {
    let Some(boss) = l.boss() else {
        return Err(Invalid::MissingRole("boss"));
    };
    if l.find(Role::Entrance).is_none() {
        return Err(Invalid::MissingRole("entrance"));
    }
    if let Some(&(i, d)) = l.open_exits().first() {
        return Err(Invalid::OpenExit(i, d));
    }
    if let Some(&(i, d)) = l.bad_links().first() {
        return Err(Invalid::BadLink(i, d));
    }
    let entrance = l.entrance();
    let seen = l.reachable_from(entrance, None);
    if let Some(i) = seen.iter().position(|&s| !s) {
        return Err(Invalid::Unreachable(i));
    }
    // The boss room is entered from its left: whatever lies past its right
    // side must not be reachable without walking through the boss room.
    if let Some(r) = l.rooms[boss].right {
        let before = l.reachable_from(entrance, Some(boss));
        if before[r] {
            return Err(Invalid::BossFromRight);
        }
    }
    if rules.require_thunderbird {
        if let Some(tb) = l.find(Role::Thunderbird) {
            let without = l.reachable_from(entrance, Some(tb));
            if without[boss] {
                return Err(Invalid::ThunderbirdBypass);
            }
        }
    }
    if rules.boss_min_distance > 0 {
        let d = l.distances()[boss];
        if d < rules.boss_min_distance {
            return Err(Invalid::BossTooClose(d));
        }
    }
    if rules.drops != DropRule::Anything {
        for (i, p) in l.rooms.iter().enumerate() {
            let Some(t) = p.drop else { continue };
            let from = l.reachable_from(t, None);
            let ok = from[entrance] || (rules.drops == DropRule::EntranceOrBoss && from[boss]);
            if !ok {
                return Err(Invalid::DropTrap(i));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::palaces::rooms::{Group, Needs, Shape};

    /// A test room with `shape` and `role`.
    pub fn room(map: u8, role: Role, shape: Shape) -> Room {
        Room {
            key: RoomKey {
                group: Group::A,
                map,
            },
            palace: 1,
            role,
            sideview: 0x8000 + u16::from(map),
            enemies: 0x7000,
            nibble: 0xF,
            conn: [0xFC, 0, 0, 0xFC],
            drop_bytes: if shape.drop.is_some() { 2 } else { 0 },
            pages: 4,
            elevator: if shape.up || shape.down {
                Some(1)
            } else {
                None
            },
            shape,
            item: None,
            keys: 0,
            doors: 0,
            needs: Needs::NONE,
            breakable: false,
        }
    }

    pub fn lr() -> Shape {
        Shape {
            left: true,
            right: true,
            ..Shape::default()
        }
    }

    pub fn only(d: Dir) -> Shape {
        let mut s = Shape::default();
        match d {
            Dir::Left => s.left = true,
            Dir::Right => s.right = true,
            Dir::Up => s.up = true,
            Dir::Down => s.down = true,
            Dir::Drop => s.drop = Some(1),
        }
        s
    }

    fn line() -> Layout {
        // entrance (down) -> up/right room -> boss (left)
        let mut l = Layout {
            palace: 1,
            rooms: vec![
                Placed::new(room(0, Role::Entrance, only(Dir::Down))),
                Placed::new(room(
                    1,
                    Role::Normal,
                    Shape {
                        up: true,
                        right: true,
                        ..Shape::default()
                    },
                )),
                Placed::new(room(2, Role::Boss, only(Dir::Left))),
            ],
            style: "test",
            boss_continues: false,
        };
        l.link(0, Dir::Down, 1);
        l.link(1, Dir::Right, 2);
        l
    }

    fn rules() -> Rules {
        Rules {
            require_thunderbird: false,
            boss_min_distance: 0,
            drops: DropRule::EntranceOrBoss,
        }
    }

    #[test]
    fn valid_line() {
        let l = line();
        assert_eq!(validate(&l, &rules()), Ok(()));
        assert_eq!(l.distances(), vec![0, 1, 2]);
        let r = Rules {
            boss_min_distance: 3,
            ..rules()
        };
        assert_eq!(validate(&l, &r), Err(Invalid::BossTooClose(2)));
    }

    #[test]
    fn open_and_bad_links_are_caught() {
        let mut l = line();
        l.rooms[1].right = None;
        assert!(matches!(
            validate(&l, &rules()),
            Err(Invalid::OpenExit(1, Dir::Right))
        ));
        let mut l = line();
        l.rooms[0].set(Dir::Right, Some(2));
        assert!(matches!(
            validate(&l, &rules()),
            Err(Invalid::BadLink(0, Dir::Right))
        ));
    }

    #[test]
    fn drop_traps() {
        let mut l = line();
        // A drop from room 1 into a landing dead end that leads nowhere.
        l.rooms[1].room.shape.drop = Some(2);
        let mut z = room(3, Role::Normal, Shape::default());
        z.shape.zone = Some(1);
        l.rooms.push(Placed::new(z));
        l.rooms[1].drop = Some(3);
        assert_eq!(validate(&l, &rules()), Err(Invalid::DropTrap(1)));
        let r = Rules {
            drops: DropRule::Anything,
            ..rules()
        };
        assert_eq!(validate(&l, &r), Ok(()));
    }

    #[test]
    fn thunderbird_must_block() {
        let mut l = line();
        l.rooms[1].room.role = Role::Thunderbird;
        let r = Rules {
            require_thunderbird: true,
            ..rules()
        };
        assert_eq!(validate(&l, &r), Ok(()));
        // add a bypass: entrance gains a right exit straight to the boss
        l.rooms[0].room.shape.right = true;
        l.rooms[2].room.shape.left = true;
        l.rooms[0].right = Some(2);
        assert_eq!(validate(&l, &r), Err(Invalid::ThunderbirdBypass));
    }
}
