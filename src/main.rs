use iced::{
    futures::SinkExt,
    stream, time,
    widget::{
        button, column, container, horizontal_rule, hover, progress_bar, responsive, row, text,
        text_input, tooltip, Row, Space,
    },
    Alignment, Element, Length, Subscription, Task,
};
use rodio::{
    source::{Buffered, SamplesConverter},
    Decoder, OutputStream, OutputStreamHandle, Source,
};
use std::{any::Any, sync::LazyLock};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    spawn,
    sync::mpsc::{channel, Sender},
    task::JoinHandle,
    time::{sleep, Instant},
};
use tooltip::Position::{Bottom, FollowCursor};
use tracing::{debug, error, info};

const E_CLICK: &[u8] = include_bytes!("../assets/e-click.wav");
const E_FLAT_CLICK: &[u8] = include_bytes!("../assets/e-flat-click.wav");
static E_CLICK_SOURCE: LazyLock<Buffered<SamplesConverter<Decoder<Cursor<&'static [u8]>>, f32>>> =
    LazyLock::new(|| {
        Decoder::new(Cursor::new(E_CLICK))
            .unwrap()
            .convert_samples()
            .buffered()
    });
static E_FLAT_CLICK_SOURCE: LazyLock<
    Buffered<SamplesConverter<Decoder<Cursor<&'static [u8]>>, f32>>,
> = LazyLock::new(|| {
    Decoder::new(Cursor::new(E_FLAT_CLICK))
        .unwrap()
        .convert_samples()
        .buffered()
});

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    iced::application("Metronome", Metronome::update, Metronome::view)
        .window_size((460.0, 340.0))
        .subscription(Metronome::subscription)
        .run_with(Metronome::new)
}

struct NotesConf {
    numerator: u8,
    denominator: u8,
    notes_sounds: Vec<Buffered<SamplesConverter<Decoder<Cursor<&'static [u8]>>, f32>>>,
}
impl NotesConf {
    fn new(numerator: u8, denominator: u8) -> Self {
        NotesConf {
            numerator,
            denominator,
            notes_sounds: Self::generate_notes_sounds(numerator),
        }
    }
    fn generate_notes_sounds(
        numerator: u8,
    ) -> Vec<Buffered<SamplesConverter<Decoder<Cursor<&'static [u8]>>, f32>>> {
        (0..numerator)
            .map(|e| {
                if e == 0 {
                    E_CLICK_SOURCE.clone()
                } else {
                    E_FLAT_CLICK_SOURCE.clone()
                }
            })
            .collect()
    }
}
struct Metronome {
    play_sender: Option<Sender<ChannelMessage>>,
    notes: Arc<Mutex<NotesConf>>,
    is_playing: bool,
    bpm: u32,
    volume: f32,
    stream: (OutputStream, OutputStreamHandle),
    instant: Option<Instant>,
    sleep_duration: Duration,
}

#[derive(Debug, Clone)]
enum Message {
    ChannelConnected(Sender<ChannelMessage>),
    StartPlaying,
    StopPlaying,
    BPMChanged(String),
    NumeratorChanged(String),
    DenominatorChanged(String),
    InstantGot(Instant),
    Tick,
}

impl Metronome {
    fn new() -> (Self, Task<Message>) {
        (
            Metronome {
                play_sender: None,
                bpm: 120,
                volume: 0.4,
                notes: Arc::new(Mutex::new(NotesConf::new(4, 4))),
                is_playing: false,
                stream: OutputStream::try_default().unwrap(),
                instant: None,
                sleep_duration: Duration::from_secs_f64((60.0 / 120_f64) * (4.0 / 4_f64)),
            },
            Task::none(),
        )
    }
    fn view(&self) -> Element<Message> {
        //todo: 音频组切换
        let top = row![
            tooltip("setting(icon todo)", "settings", FollowCursor),
            // pick_list((0..100).collect::<Vec<_>>(), Some(2)).width(Length::Fill),
            if self.is_playing {
                button("stop").on_press(Message::StopPlaying)
            } else {
                button("play").on_press(Message::StartPlaying)
            },
            tooltip(button("edit"), "Edit the voices", FollowCursor)
        ]
        .spacing(20);
        let notes_locked = self.notes.lock().unwrap();
        let config = row![
            tooltip(
                hover(
                    text(self.bpm.to_string())
                        .width(Length::Fixed(95.0))
                        .height(Length::Fixed(33.0))
                        .center(),
                    text_input("BPM", &self.bpm.to_string())
                        .on_input(Message::BPMChanged)
                        .width(Length::Fixed(95.0))
                        .align_x(Alignment::Center)
                ),
                "BPM",
                Bottom
            ),
            Space::with_width(Length::Fill),
            tooltip(button("TAP"), "Random a BPM value", FollowCursor),
            Space::with_width(Length::Fill),
            row![
                tooltip(
                    hover(
                        text(notes_locked.numerator.to_string())
                            .width(Length::Fixed(40.0))
                            .height(Length::Fixed(33.0))
                            .center(),
                        text_input("", notes_locked.numerator.to_string().as_str())
                            .on_input(Message::NumeratorChanged)
                            .width(Length::Fixed(40.0))
                            .align_x(Alignment::Center)
                    ),
                    "Number of beats per measure",
                    Bottom
                ),
                "/",
                tooltip(
                    hover(
                        text(notes_locked.denominator.to_string())
                            .width(Length::Fixed(40.0))
                            .height(Length::Fixed(33.0))
                            .center(),
                        text_input("", notes_locked.denominator.to_string().as_str())
                            .on_input(Message::DenominatorChanged)
                            .width(Length::Fixed(40.0))
                            .align_x(Alignment::Center)
                    ),
                    "Type of note which equals 1 beat",
                    Bottom
                )
            ]
            .align_y(Alignment::Center)
        ]
        .align_y(Alignment::Center);

        let mut vec_col = column![];
        let mut vec_row = Row::with_capacity(4);
        let numerator = notes_locked.numerator;
        for i in 1..=numerator {
            if i % 4 == 0 {
                vec_row = vec_row.push(responsive(move |size| {
                    progress_bar(
                        self.sleep_duration.as_secs_f32() * (i - 1) as f32
                            ..=self.sleep_duration.as_secs_f32() * i as f32,
                        if let Some(i) = self.instant {
                            i.elapsed().as_secs_f32()
                        } else {
                            0.0
                        },
                    )
                    .height(size.width)
                    .into()
                }));
                vec_col = vec_col.push(vec_row.spacing(8));
                vec_row = Row::with_capacity(4);
            } else {
                vec_row = vec_row.push(responsive(move |size| {
                    progress_bar(
                        self.sleep_duration.as_secs_f32() * (i - 1) as f32
                            ..=self.sleep_duration.as_secs_f32() * i as f32,
                        if let Some(i) = self.instant {
                            i.elapsed().as_secs_f32()
                        } else {
                            0.0
                        },
                    )
                    .height(size.width)
                    .into()
                }));
                if i == numerator {
                    vec_col = vec_col.push(vec_row.spacing(8));
                    break;
                }
            }
        }

        let beats_progress = vec_col.spacing(5);

        container(column![top, horizontal_rule(0.5), config, beats_progress].spacing(10))
            .padding(5)
            .into()
    }
    fn update(&mut self, msg: Message) -> Task<Message> {
        let restart_task =
            Task::done(Message::StopPlaying).chain(Task::done(Message::StartPlaying));
        match msg {
            Message::ChannelConnected(s) => {
                let s1 = s.clone();
                let handle = self.stream.1.clone();
                self.play_sender = Some(s);
                spawn(async move { s1.send(ChannelMessage::StreamHandle(handle)).await.unwrap() });
            }
            Message::StartPlaying => {
                let play_sender = self.play_sender.clone().unwrap();
                let notesconf = self.notes.clone();
                let bpm = self.bpm;
                let volume = self.volume;
                self.sleep_duration = Duration::from_secs_f64(
                    (60.0 / bpm as f64) * (4.0 / self.notes.lock().unwrap().denominator as f64),
                );
                self.is_playing = true;
                spawn(async move {
                    play_sender
                        .send(ChannelMessage::StartPlay {
                            notesconf,
                            bpm,
                            volume,
                        })
                        .await
                        .unwrap();
                });
            }
            Message::StopPlaying => {
                let play_sender = self.play_sender.clone().unwrap();
                self.is_playing = false;
                self.instant = None;
                spawn(async move {
                    play_sender.send(ChannelMessage::StopPlay).await.unwrap();
                });
            }
            Message::BPMChanged(s) => match s.parse() {
                Ok(i) => {
                    self.bpm = i;
                    if self.is_playing {
                        return restart_task;
                    }
                }
                Err(e) => error!("{}", e),
            },
            Message::NumeratorChanged(s) => match s.parse() {
                Ok(i) => {
                    let mut locked = self.notes.lock().unwrap();
                    locked.numerator = i;
                    locked.notes_sounds = NotesConf::generate_notes_sounds(i);
                    if self.is_playing {
                        return restart_task;
                    };
                }
                Err(e) => error!("{}", e),
            },
            Message::DenominatorChanged(s) => match s.parse() {
                Ok(i) => {
                    self.notes.lock().unwrap().denominator = i;
                    if self.is_playing {
                        return restart_task;
                    };
                }
                Err(e) => error!("{}", e),
            },
            Message::InstantGot(i) => self.instant = Some(i),
            Message::Tick => {}
        }
        Task::none()
    }
    fn subscription(&self) -> iced::Subscription<Message> {
        let tick = if self.is_playing {
            time::every(Duration::from_millis(10)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };
        Subscription::batch(vec![Subscription::run(create_stream), tick])
    }
}
fn create_stream() -> impl iced::futures::Stream<Item = Message> {
    stream::channel(100, |mut output| async move {
        let (sender, mut receiver) = channel(10);
        output
            .send(Message::ChannelConnected(sender))
            .await
            .unwrap();

        let mut stream_handle = None;

        // 接收 StreamHandle
        while let Some(ChannelMessage::StreamHandle(v)) = receiver.recv().await {
            debug!("Got OutputStreamHandle: {:?}", v.type_id());
            stream_handle = Some(v);
            break;
        }

        let stream_handle = stream_handle.unwrap();
        let mut play_future: Option<JoinHandle<()>> = None;

        while let Some(msg) = receiver.recv().await {
            match msg {
                ChannelMessage::StartPlay {
                    notesconf,
                    bpm,
                    volume,
                } => {
                    info!("Start playing");

                    let locked = notesconf.lock().unwrap();
                    let denominator = locked.denominator;
                    let notes = locked.notes_sounds.clone();
                    drop(locked);

                    // 如果有正在播放的任务，先中止
                    if let Some(f) = play_future.take() {
                        f.abort();
                    }

                    let stream_handle_clone = stream_handle.clone();
                    let notes_clone = notes.clone();

                    let sleep_duration =
                        Duration::from_secs_f64((60.0 / bpm as f64) * (4.0 / denominator as f64));

                    let mut output_ = output.clone();
                    // 启动播放任务
                    play_future = Some(spawn(async move {
                        loop {
                            for (index, source) in notes_clone.iter().enumerate() {
                                // debug!("tick...");
                                if index == 0 {
                                    output_
                                        .send(Message::InstantGot(Instant::now()))
                                        .await
                                        .unwrap();
                                }
                                // dbg!(source.clone().amplify(volume).total_duration());
                                stream_handle_clone
                                    .play_raw(source.clone().amplify(volume))
                                    .unwrap();
                                sleep(sleep_duration).await;
                            }
                        }
                    }));
                }
                ChannelMessage::StopPlay => {
                    info!("Stop playing");
                    // 停止播放的逻辑
                    if let Some(f) = play_future.take() {
                        f.abort();
                    }
                }
                ChannelMessage::StreamHandle(_) => {
                    // 处理新的 StreamHandle 的逻辑
                }
            }
        }
    })
}

enum ChannelMessage {
    StreamHandle(OutputStreamHandle),
    StartPlay {
        notesconf: Arc<Mutex<NotesConf>>,
        bpm: u32,
        volume: f32,
    },
    StopPlay,
}
