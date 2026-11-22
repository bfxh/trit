use super::Error;

pub(crate) fn add_tokens(left: u64, right: u64, task: Option<&str>) -> Result<u64, Error> {
    left.checked_add(right).ok_or_else(|| Error::TokenOverflow {
        task: task.map(str::to_owned),
        left,
        right,
    })
}
