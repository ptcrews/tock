const BITMAP_SIZE: usize = 20;

pub struct Bitmap {
    map: [u8; BITMAP_SIZE],
}

impl Bitmap {
    pub fn new() -> Bitmap {
        Bitmap {
            map: [0; BITMAP_SIZE]
        }
    }

    pub fn clear(&mut self) {
        for i in 0..self.map.len() {
            self.map[i] = 0;
        }
    }

    // TODO: Confirm this is correct
    pub fn clear_bit(&mut self, idx: usize) {
        let map_idx = idx / 8;
        self.map[map_idx] &= !(1 << (idx % 8));
    }

    // TODO: Confirm this is correct
    pub fn set_bit(&mut self, idx: usize) {
        let map_idx = idx / 8;
        self.map[map_idx] |= 1 << (idx % 8);
    }

    // Returns true if successfully set bits, returns false if the bits
    // overlapped with already set bits
    // Note that each bit represents a multiple of 8 bytes (as everything
    // must be in 8-byte groups), and thus we can store 8*8 = 64 "bytes" per
    // byte in the bitmap.
    // TODO: Check the return bool is set correctly
    pub fn set_bits(&mut self, start_idx: usize, end_idx: usize) -> bool {
        if start_idx > end_idx {
            return false;
        }
        let start_map_idx = start_idx / 8;
        let end_map_idx = end_idx / 8;
        let first = 0xff << (start_idx % 8);
        let second = 0xff >> (8 - (end_idx % 8));
        if start_map_idx == end_map_idx {
            let result = (self.map[start_map_idx] & (first & second)) == 0;
            self.map[start_map_idx] |= first & second;
            result
        } else {
            let mut result = (self.map[start_map_idx] & first) == 0;
            result = result && ((self.map[end_map_idx] & second) == 0);
            self.map[start_map_idx] |= first;
            self.map[end_map_idx] |= second;
            for i in start_map_idx + 1..end_map_idx {
                result = result && (self.map[i] == 0);
                self.map[i] = 0xff;
            }
            result
        }
    }

    pub fn is_complete(&self, total_length: usize) -> bool {
        let mut result = true;
        for i in 0..total_length / 8 {
            result = result && (self.map[i] == 0xff);
        }
        let mask = 0xff >> (8 - (total_length % 8));
        result = result && (self.map[total_length / 8] == mask);
        result
    }
}
