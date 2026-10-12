use super::*;

#[test]
fn handoff_finds_original_index_and_rejects_missing_or_ambiguous_tracks() {
    let mut list = Playlist::new().unwrap();
    list.push(&track(0)).unwrap();
    let mut duplicate = track(0);
    duplicate.playlist_index = Some(8);
    list.push(&duplicate).unwrap();
    list.push(&track(9)).unwrap();
    assert_eq!(list.locate("00000000000", Some(1)).unwrap(), 0);
    assert_eq!(list.locate("00000000000", Some(8)).unwrap(), 1);
    assert_eq!(list.locate("00000000009", Some(2)).unwrap(), 2);
    assert!(list.locate("00000000000", None).is_err());
    assert!(list.locate("00000000000", Some(4)).is_err());
    assert!(list.locate("missing", Some(1)).is_err());
}
fn track(index: u64) -> Track {
    Track {
        playlist_index: Some(index + 1),
        metadata: VideoMetadata {
            id: format!("{index:011}"),
            title: format!("Track {index}"),
            uploader: None,
            duration_seconds: None,
        },
        unavailable: false,
        failed: false,
    }
}
fn playlist(len: u64) -> Playlist {
    let mut list = Playlist::new().unwrap();
    for i in 0..len {
        list.push(&track(i)).unwrap();
    }
    list
}
#[test]
fn sequential_repeat_previous_and_selection() {
    let mut order = Order::new(3);
    assert_eq!(order.current(), Some(0));
    assert_eq!(order.previous(), None);
    assert_eq!(order.next(true, false), Some(1));
    assert_eq!(order.previous(), Some(0));
    order.select(2).unwrap();
    assert_eq!(order.next(true, false), None);
    order.configure(false, Repeat::All, 1);
    assert_eq!(order.next(true, false), Some(0));
    assert_eq!(order.previous(), Some(2));
    order.configure(false, Repeat::One, 1);
    assert_eq!(order.next(true, false), Some(2));
    assert_eq!(order.next(false, false), Some(0));
    assert!(order.select(3).is_err());
    assert_eq!(Order::new(0).next(true, false), None);
}
#[test]
fn shuffle_is_a_complete_reversible_permutation_with_manual_selection() {
    for n in 1..100 {
        for seed in 0..10 {
            let mut order = Order::new(n);
            order.configure(true, Repeat::Off, seed);
            let mut visited = vec![order.current().unwrap()];
            while let Some(index) = order.next(false, false) {
                visited.push(index);
            }
            assert_eq!(visited.len(), n as usize);
            for index in visited.iter().rev().skip(1) {
                assert_eq!(order.previous(), Some(*index));
            }
            visited.sort_unstable();
            assert_eq!(visited, (0..n).collect::<Vec<_>>());
            for i in 0..n {
                order.select(i).unwrap();
                assert_eq!(order.current(), Some(i));
            }
            order.configure(false, Repeat::All, 2);
            assert_eq!(order.current(), Some(n - 1));
        }
    }
}
#[test]
fn failures_skip_and_stop_after_one_sweep_even_with_repeat_one_or_all() {
    for repeat in [Repeat::Off, Repeat::One, Repeat::All] {
        let mut queue = Queue::new(playlist(3));
        queue.order.configure(false, repeat, 1);
        assert_eq!(queue.failed().unwrap(), Some(1));
        assert_eq!(queue.failed().unwrap(), Some(2));
        assert_eq!(queue.failed().unwrap(), None);
        assert_eq!(queue.skipped, 3);
        assert!(queue.playlist.page(0).unwrap().iter().all(|t| t.failed));
    }
}
#[test]
fn disk_index_pages_are_bounded_and_removed_on_drop() {
    let mut list = playlist(10000);
    let path = list.path.clone();
    assert_eq!(list.page(0).unwrap().len(), PAGE_SIZE);
    assert_eq!(list.page(9999).unwrap().len(), 1);
    assert!(list.page(u64::MAX).unwrap().is_empty());
    assert_eq!(list.track(9876).unwrap().metadata.title, "Track 9876");
    list.mark_failed(9876, true).unwrap();
    assert!(list.track(9876).unwrap().failed);
    assert!(std::mem::size_of::<Playlist>() < 256);
    drop(list);
    assert!(!path.exists());
    // The order has the same fixed footprint for huge playlists.
    let mut order = Order::new(1_000_000_000_000);
    order.configure(true, Repeat::All, 17);
    order.select(900_000_000_000).unwrap();
    assert_eq!(order.current(), Some(900_000_000_000));
}
