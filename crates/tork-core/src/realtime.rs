//! A small broadcast hub for fan-out messaging (chat rooms, live feeds).
//!
//! A [`Hub`] holds named [`Room`]s, each backed by a `tokio::sync::broadcast`
//! channel: every message sent to a room reaches all of its current subscribers.
//! A `Hub` is cheap to clone, so it is typically held as an injected resource.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// Default per-room channel capacity (buffered messages before lag).
const DEFAULT_CAPACITY: usize = 256;

/// A registry of broadcast [`Room`]s keyed by name.
pub struct Hub<M> {
    rooms: Arc<Mutex<HashMap<String, Arc<broadcast::Sender<M>>>>>,
    capacity: usize,
}

impl<M> Clone for Hub<M> {
    fn clone(&self) -> Self {
        Self {
            rooms: self.rooms.clone(),
            capacity: self.capacity,
        }
    }
}

impl<M: Clone + Send + 'static> Default for Hub<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Clone + Send + 'static> Hub<M> {
    /// Creates an empty hub with the default room capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Creates an empty hub whose rooms buffer up to `capacity` messages.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            capacity,
        }
    }

    /// Returns the room with the given id, creating it if it does not exist.
    ///
    /// Creating a new room first evicts any rooms that have become dead — no
    /// subscribers and no outstanding [`Room`] handles — so a server that mints
    /// rooms with dynamic names (per user or session) does not grow unboundedly.
    pub fn room(&self, id: impl Into<String>) -> Room<M> {
        let id = id.into();
        let mut rooms = self.rooms.lock().unwrap_or_else(|p| p.into_inner());
        let sender = match rooms.get(&id) {
            Some(sender) => Arc::clone(sender),
            None => {
                // A dead room has only the map's reference (strong_count == 1)
                // and no receivers; keeping any with live handles or subscribers
                // avoids dropping a room another task is about to use or send to.
                rooms.retain(|_, s| Arc::strong_count(s) > 1 || s.receiver_count() > 0);
                let sender = Arc::new(broadcast::channel(self.capacity).0);
                rooms.insert(id, Arc::clone(&sender));
                sender
            }
        };
        Room { sender }
    }

    /// Returns the number of rooms currently held by the hub.
    pub fn room_count(&self) -> usize {
        self.rooms.lock().unwrap_or_else(|p| p.into_inner()).len()
    }
}

/// A single broadcast room: send to all subscribers, or subscribe to receive.
pub struct Room<M> {
    sender: Arc<broadcast::Sender<M>>,
}

impl<M> Clone for Room<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<M: Clone + Send + 'static> Room<M> {
    /// Subscribes to the room, receiving every subsequent broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<M> {
        self.sender.subscribe()
    }

    /// Broadcasts a message, returning the number of subscribers it reached.
    pub fn broadcast(&self, message: M) -> usize {
        self.sender.send(message).unwrap_or(0)
    }

    /// Returns the current number of subscribers.
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_reaches_every_subscriber() {
        let hub = Hub::<i32>::new();
        let room = hub.room("general");
        let mut first = room.subscribe();
        let mut second = room.subscribe();

        assert_eq!(room.subscribers(), 2);
        assert_eq!(room.broadcast(42), 2);
        assert_eq!(first.recv().await.unwrap(), 42);
        assert_eq!(second.recv().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn the_same_id_returns_the_same_room() {
        let hub = Hub::<i32>::new();
        let mut receiver = hub.room("a").subscribe();
        // A separate handle to the same room id shares the channel.
        assert_eq!(hub.room("a").broadcast(7), 1);
        assert_eq!(receiver.recv().await.unwrap(), 7);
    }

    #[test]
    fn broadcast_with_no_subscribers_reaches_nobody() {
        let hub = Hub::<i32>::new();
        assert_eq!(hub.room("empty").broadcast(1), 0);
    }

    #[tokio::test]
    async fn dead_rooms_are_evicted_when_a_new_room_is_created() {
        let hub = Hub::<i32>::new();
        {
            let room = hub.room("a");
            let _subscriber = room.subscribe();
            assert_eq!(hub.room_count(), 1);
        } // both the room handle and the receiver drop here, so "a" is now dead

        // Creating a different room evicts the dead one first.
        let _b = hub.room("b");
        assert_eq!(
            hub.room_count(),
            1,
            "the dead room should have been evicted"
        );
    }

    #[tokio::test]
    async fn rooms_with_live_handles_or_subscribers_are_kept() {
        let hub = Hub::<i32>::new();

        // A held room handle (a producer) keeps the room alive.
        let producer = hub.room("kept-handle");
        // A held subscriber keeps its room alive even without a handle.
        let _subscriber = hub.room("kept-sub").subscribe();
        // A room with neither is dead.
        drop(hub.room("dead"));

        // Creating a new room triggers eviction; only the dead one goes.
        let _new = hub.room("new");
        assert_eq!(hub.room_count(), 3, "kept-handle, kept-sub, and new remain");

        // The producer's handle still addresses the same channel.
        let mut rx = hub.room("kept-handle").subscribe();
        assert_eq!(producer.broadcast(7), 1);
        assert_eq!(rx.recv().await.unwrap(), 7);
    }
}
