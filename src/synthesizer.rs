use rsunimrcp_engine::Engine;
use rsunimrcp_sys::headers::SynthHeaders;
use std::{
    io::{Read, Write},
    sync::{mpsc, Arc},
};
use tokio::io::AsyncReadExt;

#[derive(Debug)]
pub struct Synthesizer {
    engine: Arc<Engine>,
    load_complete: bool,
    audio_buf: Vec<u8>,
    have_read: usize,
    data_channel: (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>),
}

impl Synthesizer {
    pub fn leaked(engine: Arc<Engine>) -> *mut Self {
        let synthsizer = Self {
            engine,
            load_complete: false,
            audio_buf: vec![],
            have_read: 0,
            data_channel: mpsc::channel(),
        };
        Box::into_raw(Box::new(synthsizer))
    }

    pub unsafe fn destroy(this: *mut Self) {
        drop(Box::from_raw(this));
    }

    pub fn prepare(&mut self, headers: SynthHeaders) {
        self.load_complete = false;
        self.synthesize(headers);
    }

    pub fn reset(&mut self) {
        self.load_complete = false;
        self.audio_buf.clear();
        self.have_read = 0;
    }
}

impl Read for Synthesizer {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        if self.load_audio() {
            let src = self.src_for_frame(buf.len());
            buf.write(src)
        } else {
            buf.fill(0);
            Ok(buf.len())
        }
    }
}

impl Synthesizer {
    fn load_audio(&mut self) -> bool {
        if !self.load_complete {
            let rx = &self.data_channel.1;
            match rx.try_recv() {
                Ok(bytes) => {
                    self.have_read = 0;
                    self.load_complete = true;
                    self.audio_buf = bytes;
                    log::info!("Received {} bytes", self.audio_buf.len());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.load_complete = false;
                }
                e @ Err(mpsc::TryRecvError::Disconnected) => {
                    self.have_read = 0;
                    self.load_complete = true;
                    self.audio_buf = vec![];
                    log::error!("Failed to receieve TTS: {:?}", e);
                }
            }
        }
        self.load_complete
    }

    fn src_for_frame(&mut self, size: usize) -> &[u8] {
        let old_have_read = self.have_read;
        let mut new_have_read = old_have_read + size;
        if new_have_read > self.audio_buf.len() {
            new_have_read = self.audio_buf.len();
        }
        self.have_read = new_have_read;
        &self.audio_buf[old_have_read..new_have_read]
    }

    fn synthesize(&mut self, headers: SynthHeaders) {
        let tx = self.data_channel.0.clone();
        self.engine
            .async_handle()
            .spawn(connect(headers, self.engine.filename().to_owned(), tx));
    }
}

async fn connect(headers: SynthHeaders, filename: String, tx: mpsc::Sender<Vec<u8>>) {
    log::info!("Connect to audio source with headers: {:?}", headers);
    let mut audio = Vec::new();
    let Ok(mut input) = tokio::fs::File::open(&filename).await else {
        log::error!("{:?} is unavailable.", filename);
        return;
    };
    match input.read_to_end(&mut audio).await {
        Ok(_) => {
            let _ = tx.send(audio);
        }
        Err(e) => {
            log::error!("Failed to read from {:?}. {:?}", filename, e);
        }
    }
}
