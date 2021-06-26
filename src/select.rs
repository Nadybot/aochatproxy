use std::{
    future::Future,
    iter::FromIterator,
    mem,
    pin::Pin,
    task::{Context, Poll},
};

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct SelectAll<Fut> {
    inner: Vec<Fut>,
}

impl<Fut: Unpin> Unpin for SelectAll<Fut> {}

pub fn select_all<I>(iter: I) -> SelectAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future + Unpin,
{
    SelectAll {
        inner: iter.into_iter().collect(),
    }
}

impl<Fut: Future + Unpin> Future for SelectAll<Fut> {
    type Output = (Fut::Output, usize, Vec<Fut>);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let item =
            self.inner
                .iter_mut()
                .enumerate()
                .find_map(|(i, f)| match Pin::new(f).poll(cx) {
                    Poll::Pending => None,
                    Poll::Ready(e) => Some((i, e)),
                });
        match item {
            Some((idx, res)) => {
                let _ = self.inner.swap_remove(idx);
                let rest = mem::take(&mut self.inner);
                Poll::Ready((res, idx, rest))
            }
            None => Poll::Pending,
        }
    }
}

impl<Fut: Future + Unpin> FromIterator<Fut> for SelectAll<Fut> {
    fn from_iter<T: IntoIterator<Item = Fut>>(iter: T) -> Self {
        select_all(iter)
    }
}
