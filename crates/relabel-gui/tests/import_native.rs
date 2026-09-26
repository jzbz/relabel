//! The application's own importer, end to end on a real event loop: each job read
//! and decoded on a thread of its own, never the loop's, and its answer delivered
//! by the `slint::spawn_local` future `ThreadImporter` starts.
//!
//! A binary of its own, for `print_native`'s reason: an event loop needs the
//! testing backend's integration mode, which can be installed once per process
//! (i-slint-backend-testing lib.rs:48-56). Every other import test answers through
//! an importer that needs no loop. Mock time, so no Slint timer runs; the workers
//! run on real time, capped at five seconds.

#[path = "support/mod.rs"]
mod support;

use relabel_gui::import::{self, Done, Importer, Job, NoWorker, Ready, Sizing, ThreadImporter};
use relabel_render::artwork::{self, Artwork};
use relabel_render::target::Dpmm;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};
use support::tmp_dir;

type Answers = Rc<RefCell<Vec<(&'static str, ThreadId, Result<Box<Ready>, String>)>>>;

/// Where each job was read and decoded: the thread and its name.
static RAN_ON: Mutex<Vec<(ThreadId, Option<String>)>> = Mutex::new(Vec::new());

/// The application's own [`import::run`], noting the thread it runs on.
fn noted(job: &Job) -> Result<Ready, String> {
    let t = std::thread::current();
    RAN_ON
        .lock()
        .expect("nothing panics holding it")
        .push((t.id(), t.name().map(String::from)));
    import::run(job)
}

#[test]
fn the_thread_importer_delivers_through_spawn_local() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let dir = tmp_dir("import_native");
    let logo = dir.join("logo.png");
    let art = Artwork {
        w: 48,
        h: 32,
        px: (0..48 * 32)
            .map(|i| if i % 5 == 0 { 0 } else { 255 })
            .collect(),
    };
    std::fs::write(&logo, art.to_png().expect("encoded")).expect("written");
    let job = Job {
        path: logo,
        sizing: Sizing::Within(Some(304), Some(80)),
        dpmm: Dpmm::DPI_203,
    };

    let answers: Answers = Rc::default();
    // Every answer, with the thread it arrived on; the loop quits at the third.
    let heard = |what: &'static str, answers: &Answers| {
        let answers = answers.clone();
        Box::new(move |result| {
            let mut a = answers.borrow_mut();
            a.push((what, std::thread::current().id(), result));
            if a.len() == 3 {
                let _ = slint::quit_event_loop();
            }
        })
    };

    // The first answer starts another job on the same importer, which the worker
    // it answered for no longer holds up.
    let importer = Rc::new(ThreadImporter::with_run(noted));
    let restarted = Rc::new(RefCell::new(None));
    let first: Done = {
        let (importer, job, answers) = (importer.clone(), job.clone(), answers.clone());
        let (restarted, record) = (restarted.clone(), heard("first", &answers));
        Box::new(move |result| {
            record(result);
            let again = importer.start(job, heard("again", &answers));
            *restarted.borrow_mut() = Some(again.is_ok());
        })
    };
    importer.start(job.clone(), first).expect("started");
    // One worker at a time: a second is refused while the first's answer is owed.
    assert!(matches!(
        importer.start(job.clone(), heard("refused", &answers)),
        Err(NoWorker::Busy)
    ));
    let panicky = ThreadImporter::with_run(|_| panic!("boom"));
    panicky
        .start(job.clone(), heard("panicked", &answers))
        .expect("started");

    // The cap: a quit posted at five seconds, whatever has been heard.
    let stop = Arc::new(AtomicBool::new(false));
    let cap = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        })
    };
    slint::run_event_loop_until_quit().expect("the event loop runs");
    stop.store(true, Ordering::SeqCst);
    cap.join().expect("the cap ends");

    assert_eq!(*restarted.borrow(), Some(true));
    let answers = answers.take();
    let here = std::thread::current().id();
    let mut whats: Vec<&str> = answers.iter().map(|(what, ..)| *what).collect();
    whats.sort();
    assert_eq!(whats, ["again", "first", "panicked"]);
    // The decode itself was never the loop's: an answer delivered on the loop's
    // thread says nothing about where the work was done.
    let ran = RAN_ON.lock().expect("nothing panics holding it").clone();
    assert_eq!(ran.len(), 2, "{ran:?}");
    for (on, name) in ran {
        assert_ne!(on, here, "a job was decoded on the loop's thread");
        assert_eq!(name.as_deref(), Some("relabel-import"));
    }
    for (what, on, result) in answers {
        assert_eq!(on, here, "{what} is answered on the loop's thread");
        match what {
            "panicked" => {
                let e = result.expect_err("a panic is an error");
                assert!(e.contains("the importer stopped unexpectedly"), "{e}");
                assert!(e.contains("boom"), "{e}");
            }
            _ => {
                let ready = result.expect("imported");
                assert_eq!(artwork::stored_size(&ready.png), Some((48, 32)), "{what}");
            }
        }
    }
}
