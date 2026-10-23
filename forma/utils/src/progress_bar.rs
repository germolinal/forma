use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const BAR_LENGTH: usize = 50;

pub struct ProgressBar {
    title: String,
    last_progress: AtomicUsize,
    counter: AtomicUsize,
    total_count: usize,
    start: Instant,
}

impl ProgressBar {
    pub fn new(title: String, total_count: usize) -> Self {
        let ret = Self {
            title,
            total_count,
            last_progress: AtomicUsize::new(0),
            counter: AtomicUsize::new(0),
            start: Instant::now(),
        };
        ret.show_progress(0);
        ret
    }

    pub fn tic(&self) {
        // fetch_add returns the previous value, so add one before calculating
        // progress. Using that previous value was why the bar stopped short.
        let count = self.counter.fetch_add(1, Ordering::Relaxed) + 1;

        if count > self.total_count {
            eprintln!(
                "Progress bar '{}' overflowed... ticked beyond count of {}",
                self.title, self.total_count
            );
            return;
        }

        let progress = match (count * 100).checked_div(self.total_count) {
            Some(v) => v.min(100),
            None => 100,
        };
        let previous = self.last_progress.load(Ordering::Relaxed);
        let step = (100 / BAR_LENGTH).max(1);

        // Always draw the final frame, even if the final increment is smaller
        // than one bar segment.
        if progress == 100 || progress.saturating_sub(previous) >= step {
            self.last_progress.store(progress, Ordering::Relaxed);
            self.show_progress(progress);
        }
    }

    fn show_progress(&self, progress: usize) {
        let filled_length = BAR_LENGTH * progress.min(100) / 100;
        let filled = "█".repeat(filled_length);
        let empty = "░".repeat(BAR_LENGTH - filled_length);
        let colour = std::io::stdout().is_terminal();

        if colour {
            let percent_colour = if progress == 100 {
                "\x1b[92m"
            } else {
                "\x1b[93m"
            };
            print!(
                "\r    \x1b[1m{}\x1b[0m [\x1b[96m{}\x1b[0m\x1b[2;37m{}\x1b[0m] {}{:3}%\x1b[0m",
                self.title,
                filled,
                empty,
                percent_colour,
                progress.min(100)
            );
        } else {
            print!(
                "\r    {} [{}{}] {:3}%",
                self.title,
                "#".repeat(filled_length),
                "-".repeat(BAR_LENGTH - filled_length),
                progress.min(100)
            );
        }
        std::io::stdout()
            .flush()
            .expect("failed to flush progress bar");
    }

    pub fn done(&self) {
        self.counter.store(self.total_count, Ordering::Relaxed);
        self.last_progress.store(100, Ordering::Relaxed);
        self.show_progress(100);
        println!("\nDone after {} seconds", self.start.elapsed().as_secs());
    }
}
