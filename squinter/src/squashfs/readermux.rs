use std::io::{Read, Seek, SeekFrom};
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

#[derive(Debug)]
pub struct ReaderMux<R> {
    inner: Rc<RefCell<SharedReader<R>>>,
    next_client_id: usize,
}

#[derive(Debug)]
pub struct SharedReader<R> {
    inner: R,
    active_id: usize,
}

impl<R> ReaderMux<R>
where R: Read + Seek
{
    pub fn new(reader: R) -> Self {
        Self {
            inner: Rc::new(RefCell::new(SharedReader { inner: reader, active_id: 0 })),
            next_client_id: 1,
        }
    }

    pub fn client(&mut self) -> ReaderClient<R> {
        let id = self.next_client_id;
        self.next_client_id += 1;
        ReaderClient {
            inner: self.inner.clone(),
            id,
            pos: 0,
        }
    }

    pub fn into_inner(self) -> R {
        Rc::into_inner(self.inner).unwrap().into_inner().inner
    }
}

#[derive(Debug)]
pub struct ReaderClient<R> {
    inner: Rc<RefCell<SharedReader<R>>>,
    id: usize,
    pos: u64,
}

impl<R> ReaderClient<R>
where R: Seek
{
    fn activate(&self, sr: &mut RefMut<SharedReader<R>>) -> std::io::Result<u64> {
        if sr.active_id != self.id {
            sr.active_id = self.id;
            let new_pos = sr.inner.seek(SeekFrom::Start(self.pos))?;
            assert!(new_pos == self.pos);
            Ok(new_pos)
        } else {
            Ok(self.pos)
        }
    }
}

impl<R> Read for ReaderClient<R>
where R: Read + Seek
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut sr = self.inner.borrow_mut();
        self.activate(&mut sr)?;
        let r = sr.inner.read(buf);
        if let Ok(size) = r {
            self.pos += size as u64;
        }
        r
    }
}

impl<R> Seek for ReaderClient<R>
where R: Seek
{
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let mut sr = self.inner.borrow_mut();
        self.activate(&mut sr)?;
        let r = sr.inner.seek(pos);
        if let Ok(new_pos) = r {
            self.pos = new_pos;
        }
        r
    }
}