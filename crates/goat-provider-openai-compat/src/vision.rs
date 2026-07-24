pub fn known_openai_vision_model(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    if id.contains("image")
        || id.contains("audio")
        || id.contains("tts")
        || id.contains("whisper")
        || id.contains("transcribe")
        || id.contains("realtime")
        || id.contains("embedding")
        || id.contains("moderation")
        || id.contains("search")
        || id.contains("instruct")
    {
        return false;
    }
    id.starts_with("gpt-5")
        || id.starts_with("gpt-4.1")
        || id.starts_with("gpt-4o")
        || id.starts_with("o3")
        || id.starts_with("o4")
}

pub fn known_openai_compatible_vision_model(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    known_openai_vision_model(&id)
        || id.contains("vision")
        || id.contains("llava")
        || id.contains("bakllava")
        || id.contains("moondream")
        || id.contains("qwen-vl")
        || id.contains("qwen2-vl")
        || id.contains("qwen2.5-vl")
        || id.contains("qwen3-vl")
        || id.contains("minicpm-v")
        || id.contains("pixtral")
        || id.contains("internvl")
        || id.contains("cogvlm")
        || id.contains("vila")
        || id.contains("granite-vision")
        || id.contains("gemma3")
}
