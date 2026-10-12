//! Future audio engine boundary. No audio worker or device is created in this stage.
//! Implementation will use CPAL for output and Symphonia for decoding.

use std::time::Duration;

use crate::resolver::ResolvedStream;

pub trait AudioEngine: Send {
    fn load(&mut self, stream: ResolvedStream) -> Result<(), String>;
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn seek(&mut self, position: Duration) -> Result<(), String>;
    fn set_volume(&mut self, volume: f32) -> Result<(), String>;
}
