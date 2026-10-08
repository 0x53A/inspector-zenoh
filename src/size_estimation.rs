pub trait SizeEstimation {
    fn estimated_cdr_size(&self) -> usize;
}

impl SizeEstimation for bool {
    fn estimated_cdr_size(&self) -> usize {
        1
    }
}

impl SizeEstimation for u8 {
    fn estimated_cdr_size(&self) -> usize {
        1
    }
}

impl SizeEstimation for i8 {
    fn estimated_cdr_size(&self) -> usize {
        1
    }
}

impl SizeEstimation for u16 {
    fn estimated_cdr_size(&self) -> usize {
        2
    }
}

impl SizeEstimation for i16 {
    fn estimated_cdr_size(&self) -> usize {
        2
    }
}

impl SizeEstimation for u32 {
    fn estimated_cdr_size(&self) -> usize {
        4
    }
}

impl SizeEstimation for i32 {
    fn estimated_cdr_size(&self) -> usize {
        4
    }
}

impl SizeEstimation for f32 {
    fn estimated_cdr_size(&self) -> usize {
        4
    }
}

impl SizeEstimation for u64 {
    fn estimated_cdr_size(&self) -> usize {
        8
    }
}

impl SizeEstimation for i64 {
    fn estimated_cdr_size(&self) -> usize {
        8
    }
}

impl SizeEstimation for f64 {
    fn estimated_cdr_size(&self) -> usize {
        8
    }
}

impl SizeEstimation for String {
    fn estimated_cdr_size(&self) -> usize {
        4 + self.len()
    }
}
