//! 改动文件的操作（新建、改名、移动、删除）执行入口。

use std::path::Path;

use crate::app::App;

impl App {
    /// 执行访问磁盘的 `work`，再在界面线程上以其结果调用 `then`。
    pub(in crate::app) fn file_work<T: Send + 'static>(
        &mut self,
        _at: &Path,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut App, T) + 'static,
    ) {
        let outcome = work();
        then(self, outcome);
    }
}
