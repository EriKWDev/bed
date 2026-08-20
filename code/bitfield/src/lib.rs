#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Bitfield<const WORDS: usize> {
    pub words: [u64; WORDS],
}

impl<const WORDS: usize> Bitfield<WORDS> {
    pub const ZERO: Self = Self { words: [0; WORDS] };
    pub const BITS_PER_WORD: usize = 64;

    #[inline]
    pub const fn from_bit(bit: usize) -> Self {
        let mut words = [0; WORDS];
        let word_index = bit / Self::BITS_PER_WORD;
        assert!(word_index < WORDS, "bit index outside Bitfield");
        words[word_index] = 1u64 << (bit % Self::BITS_PER_WORD);
        Self { words }
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        let mut index = 0;
        while index < WORDS {
            if self.words[index] != 0 {
                return false;
            }
            index += 1;
        }
        true
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        let mut index = 0;
        while index < WORDS {
            if self.words[index] & other.words[index] != other.words[index] {
                return false;
            }
            index += 1;
        }
        true
    }

    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        let mut index = 0;
        while index < WORDS {
            if self.words[index] & other.words[index] != 0 {
                return true;
            }
            index += 1;
        }
        false
    }

    #[inline]
    pub const fn union(mut self, other: Self) -> Self {
        let mut index = 0;
        while index < WORDS {
            self.words[index] |= other.words[index];
            index += 1;
        }
        self
    }
}

impl<const WORDS: usize> Default for Bitfield<WORDS> {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

impl<const WORDS: usize> From<u64> for Bitfield<WORDS> {
    #[inline]
    fn from(value: u64) -> Self {
        let mut words = [0; WORDS];
        if WORDS != 0 {
            words[0] = value;
        }
        Self { words }
    }
}

impl<const WORDS: usize> std::fmt::LowerHex for Bitfield<WORDS> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut words = self.words.iter().rev();
        let Some(first) = words.find(|&&word| word != 0) else {
            return formatter.write_str("0");
        };
        write!(formatter, "{first:x}")?;
        for word in words {
            write!(formatter, "{word:016x}")?;
        }
        Ok(())
    }
}

impl<const WORDS: usize> std::ops::BitOr for Bitfield<WORDS> {
    type Output = Self;
    #[inline]
    fn bitor(mut self, rhs: Self) -> Self {
        self |= rhs;
        self
    }
}

impl<const WORDS: usize> std::ops::BitOrAssign for Bitfield<WORDS> {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        for index in 0..WORDS {
            self.words[index] |= rhs.words[index];
        }
    }
}

impl<const WORDS: usize> std::ops::BitAnd for Bitfield<WORDS> {
    type Output = Self;
    #[inline]
    fn bitand(mut self, rhs: Self) -> Self {
        self &= rhs;
        self
    }
}

impl<const WORDS: usize> std::ops::BitAndAssign for Bitfield<WORDS> {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        for index in 0..WORDS {
            self.words[index] &= rhs.words[index];
        }
    }
}

impl<const WORDS: usize> std::ops::BitXor for Bitfield<WORDS> {
    type Output = Self;
    #[inline]
    fn bitxor(mut self, rhs: Self) -> Self {
        self ^= rhs;
        self
    }
}

impl<const WORDS: usize> std::ops::BitXorAssign for Bitfield<WORDS> {
    #[inline]
    fn bitxor_assign(&mut self, rhs: Self) {
        for index in 0..WORDS {
            self.words[index] ^= rhs.words[index];
        }
    }
}

impl<const WORDS: usize> std::ops::Not for Bitfield<WORDS> {
    type Output = Self;
    #[inline]
    fn not(mut self) -> Self {
        for word in &mut self.words {
            *word = !*word;
        }
        self
    }
}

#[macro_export]
macro_rules! bitfield {
    (
        $(#[$attribute:meta])*
        $visibility:vis struct $name:ident: $words:literal {
            $($(#[$flag_attribute:meta])* const $flag:ident = $bit:literal;)*
        }
    ) => {
        $(#[$attribute])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        $visibility struct $name(pub $crate::Bitfield<$words>);

        impl $name {
            #[inline]
            pub const fn empty() -> Self {
                Self($crate::Bitfield::ZERO)
            }

            #[inline]
            pub const fn from_bits_retain(bits: $crate::Bitfield<$words>) -> Self {
                Self(bits)
            }

            #[inline]
            pub const fn bits(self) -> $crate::Bitfield<$words> {
                self.0
            }

            #[inline]
            pub const fn contains(self, other: Self) -> bool {
                self.0.contains(other.0)
            }

            #[inline]
            pub const fn intersects(self, other: Self) -> bool {
                self.0.intersects(other.0)
            }

            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0.is_empty()
            }

            #[inline]
            pub fn insert(&mut self, other: Self) {
                *self |= other;
            }

            #[inline]
            pub fn remove(&mut self, other: Self) {
                for index in 0..$words {
                    self.0.words[index] &= !other.0.words[index];
                }
            }

            #[inline]
            pub fn set(&mut self, other: Self, enabled: bool) {
                if enabled {
                    self.insert(other);
                } else {
                    self.remove(other);
                }
            }

            #[inline]
            pub const fn union(self, other: Self) -> Self {
                let mut words = self.0.words;
                let mut index = 0;
                while index < $words {
                    words[index] |= other.0.words[index];
                    index += 1;
                }
                Self($crate::Bitfield { words })
            }

            pub const FLAGS: &'static [(&'static str, usize)] = &[
                $((stringify!($flag), $bit),)*
            ];

            $($(#[$flag_attribute])* pub const $flag: Self = Self($crate::Bitfield::from_bit($bit));)*
        }

        impl std::ops::BitOr for $name {
            type Output = Self;
            #[inline]
            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }

        impl std::ops::BitAnd for $name {
            type Output = Self;
            #[inline]
            fn bitand(self, rhs: Self) -> Self {
                Self(self.0 & rhs.0)
            }
        }

        impl std::ops::BitXor for $name {
            type Output = Self;
            #[inline]
            fn bitxor(self, rhs: Self) -> Self {
                Self(self.0 ^ rhs.0)
            }
        }

        impl std::ops::Not for $name {
            type Output = Self;
            #[inline]
            fn not(self) -> Self {
                Self(!self.0)
            }
        }

        impl std::ops::BitOrAssign for $name {
            #[inline]
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }

        impl std::ops::BitAndAssign for $name {
            #[inline]
            fn bitand_assign(&mut self, rhs: Self) {
                self.0 &= rhs.0;
            }
        }

        impl std::ops::BitXorAssign for $name {
            #[inline]
            fn bitxor_assign(&mut self, rhs: Self) {
                self.0 ^= rhs.0;
            }
        }
    };
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    bitfield! {
        #[allow(dead_code)]
        pub struct Flags: 5 {
            const FIRST = 0;
            const WORD_TWO = 129;
            const WORD_THREE = 300;
        }
    }

    #[test]
    fn named_flags_compose_across_words() {
        let mut flags = Flags::FIRST | Flags::WORD_THREE;
        assert!(flags.contains(Flags::FIRST | Flags::WORD_THREE));
        assert!(!flags.intersects(Flags::WORD_TWO));

        flags |= Flags::WORD_TWO;
        flags ^= Flags::FIRST;
        flags &= !Flags::WORD_THREE;

        assert_eq!(flags, Flags::WORD_TWO);
        assert_eq!(
            Flags::FLAGS,
            [("FIRST", 0), ("WORD_TWO", 129), ("WORD_THREE", 300)]
        );
    }
}
