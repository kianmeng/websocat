#![allow(unused)]

extern crate websocket;
extern crate futures;
extern crate tokio_core;
#[macro_use]
extern crate tokio_io;
extern crate tokio_stdin_stdout;

#[cfg(unix)]
extern crate tokio_file_unix;
#[cfg(unix)]
extern crate tokio_signal;

use std::thread;
use std::io::stdin;
use tokio_core::reactor::Core;
use futures::future::Future;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::sync::mpsc;
use websocket::result::WebSocketError;
use websocket::{ClientBuilder, OwnedMessage};
use tokio_io::{AsyncRead,AsyncWrite};
use std::io::{Read,Write};
use std::io::Result as IoResult;

use websocket::stream::async::Stream as WsStream;
use futures::Async::{Ready, NotReady};

use tokio_io::io::copy;

use tokio_io::codec::FramedRead;
use std::fs::File;

#[cfg(unix)]
use tokio_file_unix::{File as UnixFile, StdFile};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;

type Result<T> = std::result::Result<T, Box<std::error::Error>>;

type WaitingForImplTraitFeature0 = tokio_io::codec::Framed<std::boxed::Box<websocket::async::Stream + std::marker::Send>, websocket::async::MessageCodec<websocket::OwnedMessage>>;
type WaitingForImplTraitFeature1 = futures::stream::SplitStream<WaitingForImplTraitFeature0>;
type WaitingForImplTraitFeature2 = futures::stream::SplitSink<WaitingForImplTraitFeature0>;


fn wouldblock<T>() -> std::io::Result<T> {
    Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, ""))
}
fn brokenpipe<T>() -> std::io::Result<T> {
    Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, ""))
}
fn io_other_error<E : std::error::Error + Send + Sync + 'static>(e:E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other,e)
}

mod my_copy;



struct WsReadWrapper {
    s: WaitingForImplTraitFeature1,
    debt: Option<Vec<u8>>,
}

impl AsyncRead for WsReadWrapper {

}

impl WsReadWrapper {
    fn process_message(&mut self, buf: &mut [u8], buf_in: &[u8]) -> std::result::Result<usize, std::io::Error> {
        let l = buf_in.len().min(buf.len());
        buf[..l].copy_from_slice(&buf_in[..l]);
        
        if l < buf_in.len() {
            self.debt = Some(buf_in[l..].to_vec());
        }
        
        Ok(l)
    }
}

impl Read for WsReadWrapper {
    fn read(&mut self, buf: &mut [u8]) -> std::result::Result<usize, std::io::Error> {
        if let Some(debt) = self.debt.take() {
            return self.process_message(buf, debt.as_slice())
        }
        match self.s.poll().map_err(io_other_error)? {
            Ready(Some(OwnedMessage::Close(_))) => {
                brokenpipe()
            },
            Ready(None) => {
                brokenpipe()
            }
            Ready(Some(OwnedMessage::Ping(_))) => {
                Ok(0)
                // TODO
            }
            Ready(Some(OwnedMessage::Pong(_))) => {
                Ok(0)
            }
            Ready(Some(OwnedMessage::Text(x))) => {
                self.process_message(buf, x.as_str().as_bytes())
            }
            Ready(Some(OwnedMessage::Binary(x))) => {
                self.process_message(buf, x.as_slice())
            }
            NotReady => {
                wouldblock()
            }
        }
    }
}

struct WsWriteWrapper(WaitingForImplTraitFeature2);

impl AsyncWrite for WsWriteWrapper {
    fn shutdown(&mut self) -> futures::Poll<(),std::io::Error> {
        // TODO: check this
        Ok(Ready(()))
    }
}

impl Write for WsWriteWrapper {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let om = OwnedMessage::Binary(buf.to_vec());
        match self.0.start_send(om).map_err(io_other_error)? {
            futures::AsyncSink::NotReady(_) => {
                wouldblock()
            },
            futures::AsyncSink::Ready => {
                Ok(buf.len())
            }
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        match self.0.poll_complete().map_err(io_other_error)? {
            NotReady => {
                wouldblock()
            },
            Ready(()) => {
                Ok(())
            }
        }
    }

}


fn run() -> Result<()> {
    let peeraddr = std::env::args().nth(1).ok_or("no arg")?;

    println!("Connecting to {}", peeraddr);
    let mut core = Core::new()?;
    let handle = core.handle();
    
    let si;
    let so;
    
    #[cfg(any(not(unix),feature="no_unix_stdio"))]
    {
        si = tokio_stdin_stdout::stdin(0);
        so = tokio_stdin_stdout::stdout(0);
    }
    
    #[cfg(all(unix,not(feature="no_unix_stdio")))]
    {
        let stdin  = UnixFile::new_nb(std::io::stdin())?;
        let stdout = UnixFile::new_nb(std::io::stdout())?;
    
        si = stdin.into_reader(&handle)?;
        so = stdout.into_io(&handle)?;
        
        let ctrl_c = tokio_signal::ctrl_c(&handle).flatten_stream();
        let prog = ctrl_c.for_each(|()| {
            UnixFile::raw_new(std::io::stdin()).set_nonblocking(false);
            UnixFile::raw_new(std::io::stdout()).set_nonblocking(false);
            ::std::process::exit(0);
            Ok(())
        });
        handle.spawn(prog.map_err(|_|()));
    }

    let runner = ClientBuilder::new(peeraddr.as_ref())?
        .add_protocol("rust-websocket")
        .async_connect(None, &core.handle())
        .and_then(|(duplex, _)| {
            let (sink, stream) = duplex.split();
            
            let ws_str = WsReadWrapper {
                s: stream,
                debt: None,
            };
            let ws_sin = WsWriteWrapper(sink);
            
            handle.spawn(my_copy::copy(si, ws_sin).map(|_|()).map_err(|_|()));
            my_copy::copy(ws_str, so).map_err(|e| WebSocketError::IoError(e))
        });
    core.run(runner)?;
    Ok(())
}

fn main() {
    let r = run();
    
    #[cfg(all(unix,not(feature="no_unix_stdio")))]
    {
        UnixFile::raw_new(std::io::stdin()).set_nonblocking(false);
        UnixFile::raw_new(std::io::stdout()).set_nonblocking(false);
    }
            
    if let Err(e) = r {
        eprintln!("websocat: {}", e);
        ::std::process::exit(1);
    }
}
