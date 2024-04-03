#![allow(clippy::missing_safety_doc)]

// mod config;
mod synthesizer;

use rsunimrcp_engine::RawEngine;
use rsunimrcp_sys::headers::SynthHeaders;
use rsunimrcp_sys::uni;
use rsunimrcp_sys::*;
use std::{io::Read, mem::size_of};
use synthesizer::Synthesizer;

const SYNTH_ENGINE_TASK_NAME: &[u8; 16] = b"Rust TTS-Engine\0";

pub static ENGINE_VTABLE: uni::mrcp_engine_method_vtable_t = uni::mrcp_engine_method_vtable_t {
    destroy: Some(engine_destroy),
    open: Some(engine_open),
    close: Some(engine_close),
    create_channel: Some(engine_create_channel),
};

pub const CHANNEL_VTABLE: uni::mrcp_engine_channel_method_vtable_t =
    uni::mrcp_engine_channel_method_vtable_t {
        destroy: Some(channel_destroy),
        open: Some(channel_open),
        close: Some(channel_close),
        process_request: Some(channel_process_request),
    };

pub static STREAM_VTABLE: uni::mpf_audio_stream_vtable_t = uni::mpf_audio_stream_vtable_t {
    destroy: Some(stream_destroy),
    open_rx: Some(stream_open),
    close_rx: Some(stream_close),
    read_frame: Some(stream_read),
    open_tx: None,
    close_tx: None,
    write_frame: None,
    trace: None,
};

#[repr(C)]
struct MrcpSynthEngine {
    task: *mut uni::apt_consumer_task_t,
    raw_engine: *mut RawEngine,
}

#[derive(Debug)]
#[repr(C)]
struct MrcpSynthChannel {
    custom_engine: *mut MrcpSynthEngine,
    channel: *mut uni::mrcp_engine_channel_t,
    speak_request: *mut uni::mrcp_message_t,
    stop_response: *mut uni::mrcp_message_t,
    paused: uni::apt_bool_t,
    audio_source: *mut Synthesizer,
}

#[repr(C)]
enum MrcpSynthMsgType {
    OpenChannel,
    CloseChannel,
    RequestProcess,
}

#[repr(C)]
struct MrcpSynthMsg {
    type_: MrcpSynthMsgType,
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
}

#[no_mangle]
pub static mut mrcp_plugin_version: uni::mrcp_plugin_version_t = uni::mrcp_plugin_version_t {
    major: uni::PLUGIN_MAJOR_VERSION as i32,
    minor: uni::PLUGIN_MINOR_VERSION as i32,
    patch: uni::PLUGIN_PATCH_VERSION as i32,
    is_dev: 0,
};

#[no_mangle]
pub unsafe extern "C" fn mrcp_plugin_create(pool: *mut uni::apr_pool_t) -> *mut uni::mrcp_engine_t {
    log::trace!("Going to Create TTS-Engine on pool = {:?}", pool);

    let custom_engine = uni::apr_palloc(pool, size_of::<MrcpSynthEngine>()) as *mut MrcpSynthEngine;

    (*custom_engine).raw_engine = std::ptr::null_mut() as _;

    let msg_pool = uni::apt_task_msg_pool_create_dynamic(size_of::<MrcpSynthMsg>(), pool);
    (*custom_engine).task = uni::apt_consumer_task_create(custom_engine as _, msg_pool, pool);
    if (*custom_engine).task.is_null() {
        return std::ptr::null_mut();
    }
    let task = uni::apt_consumer_task_base_get((*custom_engine).task);
    uni::apt_task_name_set(task, SYNTH_ENGINE_TASK_NAME.as_ptr() as _);
    let vtable = uni::apt_task_vtable_get(task);
    if !vtable.is_null() {
        (*vtable).process_msg = Some(synth_msg_process);
    }

    let engine = uni::mrcp_engine_create(
        uni::MRCP_SYNTHESIZER_RESOURCE as _,
        custom_engine as _,
        &ENGINE_VTABLE as _,
        pool,
    );
    log::info!("TTS-Engine Created: {:?}", engine);

    engine
}

unsafe extern "C" fn engine_destroy(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpSynthEngine;
    log::info!(
        "Destroy Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );
    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let destroyed = uni::apt_task_destroy(task);
        (*custom_engine).task = std::ptr::null_mut() as _;
        log::trace!("Task {:?} destroyed = {:?}", task, destroyed);
    }
    RawEngine::destroy((*custom_engine).raw_engine);
    uni::TRUE
}

unsafe extern "C" fn engine_open(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpSynthEngine;
    log::trace!(
        "Open Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );

    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let started = uni::apt_task_start(task);
        log::trace!("Task = {:?} started = {:?}.", task, started);
    }

    (*custom_engine).raw_engine = RawEngine::leaked(engine);
    log::info!("Opened with Engine: {:?}", (*custom_engine).raw_engine);

    inline_mrcp_engine_open_respond(engine, uni::TRUE)
}

unsafe extern "C" fn engine_close(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpSynthEngine;
    log::info!(
        "Close Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );

    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let terminated = uni::apt_task_terminate(task, uni::TRUE);
        log::trace!("Task = {:?} terminated = {:?}.", task, terminated);
    }

    inline_mrcp_engine_close_respond(engine)
}

unsafe extern "C" fn engine_create_channel(
    engine: *mut uni::mrcp_engine_t,
    pool: *mut uni::apr_pool_t,
) -> *mut uni::mrcp_engine_channel_t {
    log::trace!("Engine {:?} is going to create a channel", engine);

    let custom_engine = (*engine).obj as *mut MrcpSynthEngine;
    let rs_engine = (*(*custom_engine).raw_engine).engine();

    let synth_channel =
        uni::apr_palloc(pool, size_of::<MrcpSynthChannel>()) as *mut MrcpSynthChannel;
    (*synth_channel).custom_engine = custom_engine;
    (*synth_channel).speak_request = std::ptr::null_mut() as _;
    (*synth_channel).stop_response = std::ptr::null_mut() as _;
    (*synth_channel).paused = uni::FALSE;
    (*synth_channel).audio_source = Synthesizer::leaked(rs_engine);

    let capabilities = inline_mpf_source_stream_capabilities_create(pool);
    inline_mpf_codec_capabilities_add(
        &mut (*capabilities).codecs as _,
        (uni::MPF_SAMPLE_RATE_8000 | uni::MPF_SAMPLE_RATE_16000) as _,
        b"LPCM\0".as_ptr() as _,
    );

    let termination = uni::mrcp_engine_audio_termination_create(
        synth_channel as _,
        &STREAM_VTABLE as _,
        capabilities,
        pool,
    );

    (*synth_channel).channel = uni::mrcp_engine_channel_create(
        engine,
        &CHANNEL_VTABLE as _,
        synth_channel as _,
        termination,
        pool,
    );

    log::info!(
        "Engine created channel = {:?} ({:6})",
        (*synth_channel).channel,
        (*(*custom_engine).raw_engine).channel_opened()
    );
    (*synth_channel).channel
}

pub unsafe extern "C" fn channel_destroy(
    channel: *mut uni::mrcp_engine_channel_t,
) -> uni::apt_bool_t {
    log::debug!("Channel {:?} destroy.", channel);
    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    Synthesizer::destroy((*synth_channel).audio_source);
    uni::TRUE
}

pub unsafe extern "C" fn channel_open(channel: *mut uni::mrcp_engine_channel_t) -> uni::apt_bool_t {
    log::info!("Channel {:?} open.", channel);
    synth_msg_signal(
        MrcpSynthMsgType::OpenChannel,
        channel,
        std::ptr::null_mut() as _,
    )
}

unsafe extern "C" fn channel_close(channel: *mut uni::mrcp_engine_channel_t) -> uni::apt_bool_t {
    log::info!("Channel {:?} close.", channel);
    synth_msg_signal(
        MrcpSynthMsgType::CloseChannel,
        channel,
        std::ptr::null_mut() as _,
    )
}

unsafe extern "C" fn channel_process_request(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::trace!("Channel {:?} process request {:?}.", channel, request);
    synth_msg_signal(MrcpSynthMsgType::RequestProcess, channel, request)
}

unsafe fn synth_channel_speak(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::trace!(
        "Process Speak request {:?} for channel {:?}",
        request,
        channel
    );
    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    let decriptor = uni::mrcp_engine_source_stream_codec_get(channel);
    if decriptor.is_null() {
        log::error!("Failed to Get Codec Descriptor {:?}", *request);
        (*response).start_line.status_code = uni::MRCP_STATUS_CODE_METHOD_FAILED;
        return uni::FALSE;
    }

    let source = (*synth_channel).audio_source;
    (*source).prepare(SynthHeaders::new(request));

    (*response).start_line.request_state = uni::MRCP_REQUEST_STATE_INPROGRESS;
    inline_mrcp_engine_channel_message_send(channel, response);
    (*synth_channel).speak_request = request;
    uni::TRUE
}

unsafe fn synth_channel_stop(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::info!("Process Stop request {:?} for {:?}", request, channel);
    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    (*synth_channel).stop_response = response;

    uni::TRUE
}

unsafe fn synth_channel_pause(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::debug!("Process Pause request {:?} for {:?}", request, channel);

    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    (*synth_channel).paused = uni::TRUE;
    inline_mrcp_engine_channel_message_send(channel, response);

    uni::TRUE
}

unsafe fn synth_channel_resume(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::debug!("Process Resume request {:?} for {:?}", request, channel);

    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    (*synth_channel).paused = uni::FALSE;
    inline_mrcp_engine_channel_message_send(channel, response);

    uni::TRUE
}

unsafe fn synth_channel_request_dispatch(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let mut processed = uni::FALSE;
    let response = uni::mrcp_response_create(request, (*request).pool);
    let method = (*request).start_line.method_id;

    log::debug!(
        "Dispatch request {:?}, method ({}) for {:?}",
        request,
        method,
        channel
    );

    match method as u32 {
        uni::SYNTHESIZER_SPEAK => processed = synth_channel_speak(channel, request, response),
        uni::SYNTHESIZER_STOP => processed = synth_channel_stop(channel, request, response),
        uni::SYNTHESIZER_PAUSE => processed = synth_channel_pause(channel, request, response),
        uni::SYNTHESIZER_RESUME => processed = synth_channel_resume(channel, request, response),
        uni::SYNTHESIZER_BARGE_IN_OCCURRED => {
            processed = synth_channel_stop(channel, request, response)
        }
        _ => {}
    }
    if processed == uni::FALSE {
        log::warn!("Unprocessed request {:?}", request);
        inline_mrcp_engine_channel_message_send(channel, response);
    }

    uni::TRUE
}

pub unsafe extern "C" fn stream_destroy(_stream: *mut uni::mpf_audio_stream_t) -> uni::apt_bool_t {
    uni::TRUE
}

pub unsafe extern "C" fn stream_open(
    _stream: *mut uni::mpf_audio_stream_t,
    _codec: *mut uni::mpf_codec_t,
) -> uni::apt_bool_t {
    uni::TRUE
}

pub unsafe extern "C" fn stream_close(_stream: *mut uni::mpf_audio_stream_t) -> uni::apt_bool_t {
    uni::TRUE
}

pub unsafe extern "C" fn stream_read(
    stream: *mut uni::mpf_audio_stream_t,
    frame: *mut uni::mpf_frame_t,
) -> uni::apt_bool_t {
    let synth_channel = (*stream).obj as *mut MrcpSynthChannel;
    let source = (*synth_channel).audio_source;

    if !(*synth_channel).stop_response.is_null() {
        inline_mrcp_engine_channel_message_send(
            (*synth_channel).channel,
            (*synth_channel).stop_response,
        );
        log::info!(
            "Stop response {:?} for {:?} finalised.",
            (*synth_channel).stop_response,
            (*synth_channel).channel
        );
        (*synth_channel).stop_response = std::ptr::null_mut() as _;
        (*synth_channel).speak_request = std::ptr::null_mut() as _;
        (*synth_channel).paused = uni::FALSE;
        (*source).reset();
        return uni::TRUE;
    }

    if !(*synth_channel).speak_request.is_null() && (*synth_channel).paused == uni::FALSE {
        let mut completed = uni::FALSE;
        let size = (*frame).codec_frame.size;
        let buffer = std::slice::from_raw_parts_mut((*frame).codec_frame.buffer as *mut u8, size);
        match (*source).read(buffer) {
            Ok(have_read) => {
                if have_read == size {
                    (*frame).type_ |= uni::MEDIA_FRAME_TYPE_AUDIO as i32;
                } else {
                    completed = uni::TRUE;
                }
            }
            Err(ref e) => {
                log::error!("Read from audio source with error: {}", e);
                completed = uni::TRUE;
            }
        }
        if completed == uni::TRUE {
            let message = uni::mrcp_event_create(
                (*synth_channel).speak_request,
                uni::SYNTHESIZER_SPEAK_COMPLETE as _,
                (*(*synth_channel).speak_request).pool,
            );
            if !message.is_null() {
                let synth_header =
                    inline_mrcp_resource_header_prepare(message) as *mut uni::mrcp_synth_header_t;
                if !synth_header.is_null() {
                    (*synth_header).completion_cause = uni::SYNTHESIZER_COMPLETION_CAUSE_NORMAL;
                    uni::mrcp_resource_header_property_add(
                        message,
                        uni::SYNTHESIZER_HEADER_COMPLETION_CAUSE as _,
                    );
                    (*message).start_line.request_state = uni::MRCP_REQUEST_STATE_COMPLETE;
                    inline_mrcp_engine_channel_message_send((*synth_channel).channel, message);
                }
            }
            (*synth_channel).speak_request = std::ptr::null_mut() as _;
            (*source).reset();
        }
    }
    uni::TRUE
}

unsafe extern "C" fn synth_msg_signal(
    type_: MrcpSynthMsgType,
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let mut status = uni::FALSE;
    let synth_channel = (*channel).method_obj as *mut MrcpSynthChannel;
    let synth_engine = (*synth_channel).custom_engine;
    let task = uni::apt_consumer_task_base_get((*synth_engine).task);
    let msg = uni::apt_task_msg_get(task);
    if !msg.is_null() {
        (*msg).type_ = uni::TASK_MSG_USER as _;
        let synth_msg = (*msg).data.as_mut_ptr() as *mut MrcpSynthMsg;
        (*synth_msg).type_ = type_;
        (*synth_msg).channel = channel;
        (*synth_msg).request = request;
        status = uni::apt_task_msg_signal(task, msg);
    }
    status
}

unsafe extern "C" fn synth_msg_process(
    _task: *mut uni::apt_task_t,
    msg: *mut uni::apt_task_msg_t,
) -> uni::apt_bool_t {
    let synth_msg = (*msg).data.as_mut_ptr() as *mut MrcpSynthMsg;
    match (*synth_msg).type_ {
        MrcpSynthMsgType::OpenChannel => {
            inline_mrcp_engine_channel_open_respond((*synth_msg).channel, uni::TRUE);
        }
        MrcpSynthMsgType::CloseChannel => {
            inline_mrcp_engine_channel_close_respond((*synth_msg).channel);
        }
        MrcpSynthMsgType::RequestProcess => {
            synth_channel_request_dispatch((*synth_msg).channel, (*synth_msg).request);
        }
    }
    uni::TRUE
}
