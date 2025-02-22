use loupe::{MemoryUsage, MemoryUsageTracker};
use loupe_derive::MemoryUsage;

use std::collections::BTreeSet;

macro_rules! assert_size_of_val_eq {
    ($expected:expr, $value:expr) => {
        assert_eq!(
            $expected,
            MemoryUsage::size_of_val(&$value, &mut BTreeSet::new())
        );
    };
}

#[test]
fn test_struct_flat() {
    #[derive(MemoryUsage)]
    struct Point {
        x: i32,
        y: i32,
    }

    assert_size_of_val_eq!(8, Point { x: 1, y: 2 });
}

#[test]
fn test_tuple() {
    #[derive(MemoryUsage)]
    struct Tuple(i32, i32);

    assert_size_of_val_eq!(8, Tuple(1, 2));
}

#[test]
fn test_struct_generic() {
    #[derive(MemoryUsage)]
    struct Generic<T>
    where
        T: MemoryUsage,
    {
        x: T,
        y: T,
    }

    assert_size_of_val_eq!(16, Generic { x: 1i64, y: 2i64 });
}

#[test]
fn test_struct_empty() {
    #[derive(MemoryUsage)]
    struct Empty;

    assert_size_of_val_eq!(0, Empty);
}

#[test]
fn test_struct_padding() {
    // This struct is packed in order <x, z, y> because 'y: i32' requires 32-bit
    // alignment but x and z do not. It starts with bytes 'x...yyyy' then adds 'z' in
    // the first place it fits producing 'xz..yyyy' and not 12 bytes 'x...yyyyz...'.
    #[derive(MemoryUsage)]
    struct Padding {
        x: i8,
        y: i32,
        z: i8,
    }

    assert_size_of_val_eq!(8, Padding { x: 1, y: 2, z: 3 });
}

#[test]
fn test_enum() {
    #[derive(MemoryUsage)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[derive(MemoryUsage)]
    enum Things {
        A,
        B(),
        C(i32),
        D { x: i32 },
        E(i32, i32),
        F { x: i32, y: i32 },
        Points(Vec<Point>),
    }

    assert_size_of_val_eq!(32, Things::A);
    assert_size_of_val_eq!(32, Things::B());
    assert_size_of_val_eq!(32, Things::C(1));
    assert_size_of_val_eq!(32, Things::D { x: 1 });
    assert_size_of_val_eq!(32, Things::E(1, 2));
    assert_size_of_val_eq!(32, Things::F { x: 1, y: 2 });

    assert_size_of_val_eq!(8, Point { x: 1, y: 2 });
    assert_size_of_val_eq!(40, vec![Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]);
    assert_size_of_val_eq!(
        48,
        Things::Points(vec![Point { x: 1, y: 2 }, Point { x: 3, y: 4 }])
    );
}
