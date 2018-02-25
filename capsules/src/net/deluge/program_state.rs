use core::cell::Cell;
use kernel::common::take_cell::TakeCell;

pub trait DelugeProgramClient {
    fn updated_page(&self);
}

pub struct ProgramState<'a> {
    unique_id: Cell<usize>,
    cur_page_num: Cell<usize>,
    page: TakeCell<'static, [u8]>,
    client: &'a DelugeProgramClient,

    //next: ListLink<'a, ProgramState<'a>>,
}

/*
 * TODO: Implement multiplexing
impl<'a> ListNode<'a, ProgramState<'a>> for ProgramState<'a> {
    fn next(&self) -> &'a ListLink<ProgramState<'a>> {
        &self.next
    }
}
*/

impl<'a> ProgramState<'a> {
    //pub fn new()

    pub fn is_page_complete(&self) -> bool {
        false
    }
}
