pub struct Stream;

pub fn fits(n: usize) -> bool {
    n < crate::socket::LIMIT
}
