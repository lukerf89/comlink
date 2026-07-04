<img src="https://r2cdn.perplexity.ai/pplx-full-logo-primary-dark%402x.png" style="height:64px;margin-right:32px"/>

# what are the newest open-source llms that are ideal for transcribing audio to text, e.g. dictation, transcription

The strongest open‑source options right now for dictation/transcription are still Whisper (esp. Faster‑Whisper variants), NVIDIA Parakeet, and the newer Voxtral / Qwen2‑Audio / FunASR family, with Whisper/Faster‑Whisper remaining the most practical default for local use.[^1][^2][^3][^4]

## Key modern open‑source STT models (2025–2026)

- **OpenAI Whisper Large V3 / V3 Turbo**
    - Multilingual, robust to noise, great for general dictation and long-form transcripts.[^2][^4]
    - Open‑sourced weights; huge ecosystem (Faster‑Whisper, whisper.cpp, Desktop GUIs like Buzz).[^3][^2]
    - Still the “safe default” for local transcription, especially when wrapped in CTranslate2/Faster‑Whisper for speed on CPU/GPU.[^3]
- **Faster‑Whisper (CTranslate2 Whisper)**
    - Optimized implementation of Whisper that gives large speedups and lower memory usage on commodity hardware.[^3]
    - Good fit for local dictation, coding‑assistant voice input, and batch meeting transcription when you care about throughput.
- **NVIDIA Parakeet TDT 0.6B (FastConformer‑TDT)**
    - Multilingual ASR with very strong WER on Hugging Face’s Open ASR leaderboard (around 6–7% average).[^2]
    - Open‑source model weights; designed for low‑latency streaming and real‑time transcription scenarios.[^2]
    - Great choice if you want GPU‑accelerated, real‑time dictation in a service oriented around EU languages.
- **Mistral Voxtral‑Mini‑4B Realtime (2026)**
    - Newer realtime ASR model in the Voxtral line, available as open source on Hugging Face.[^5][^1]
    - Targeted at meetings, voice notes, and continuous transcription, with a focus on streaming and latency.[^1]
    - Interesting for agents where you already use Mistral; you can keep stack “in‑family”.
- **Qwen2‑Audio / Qwen ASR models**
    - Multilingual audio LLMs with dedicated ASR heads; handle difficult acoustic conditions and mixed languages.[^2][^3]
    - Good for multilingual dictation (Chinese dialects + English + others) and handling background music/noise.[^3][^2]
- **FunASR pretrained models**
    - Toolkit + SOTA pretrained ASR for Mandarin and various languages; some models focused on streaming.[^3]
    - Better fit if you want deep customization and are comfortable hacking PyTorch toolkits.


## End‑to‑end open transcription tools

These wrap the models into something closer to what you’d deploy for dictation:

- **Buzz (desktop Whisper app)** – Offline GUI around Whisper for personal transcription; useful for “no infra” use but also a reference for packaging.[^3]
- **LocalAudioTran‑LLM‑Summar** – Uses local Whisper + local LLM (Phi‑4) to do transcription and summarization in one pipeline.[^6]
- Various “voice typing” tools from 2026 lists wrap Faster‑Whisper for coding agents and browser input (Agentic Coding Weekly, etc.).[^7]


## Practical recommendations by use case

- **Developer dictation / CLI + IDE voice input (your world)**
    - Whisper Large V3 via Faster‑Whisper for highest accuracy across tech jargon, plus a small local LLM to clean/punctuate commands.
    - If you want real‑time partials for interactive agents, consider Parakeet TDT or Voxtral‑Mini‑Realtime on GPU.[^5][^1][^2]
- **Enterprise meeting/call transcription**
    - Whisper Large V3 or Parakeet for accuracy, with segment timestamps and diarization added via surrounding logic.[^4][^2]
    - Qwen2‑Audio or FunASR if you have strong multilingual needs or primarily Asian languages.[^2][^3]
- **On‑device / lightweight**
    - Smaller Whisper models (base/small) in Faster‑Whisper, or the low‑parameter Parakeet variants for edge devices.[^1][^3]
    - Combine with VAD to avoid wasting compute on silence.

A concrete example stack for you: microphone → WebRTC/GRPC stream → Parakeet TDT or Voxtral‑Mini‑Realtime on a small GPU node → text events → local LLM (e.g., small open source model) for punctuation, formatting, and intent extraction → pipe into Claude Code/Perplexity or your agent harness.

What’s your primary target: local dictation on a laptop, or a small GPU service that feeds agents (e.g., in Claude Code / your orchestration stack)?
<span style="display:none">[^10][^11][^12][^13][^14][^15][^8][^9]</span>

<div align="center">⁂</div>

[^1]: https://modal.com/blog/open-source-stt

[^2]: https://openrouter.ai/collections/speech-to-text-models

[^3]: https://www.aibase.com/repos/topic/audio-transcription

[^4]: https://github.com/openai/whisper

[^5]: https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602

[^6]: https://github.com/askaresh/LocalAudioTran-LLM-Summar

[^7]: https://www.agenticcodingweekly.com/p/5-best-open-source-speech-to-text-tools-in-2026

[^8]: https://www.gladia.io/blog/best-open-source-speech-to-text-models

[^9]: https://www.youtube.com/watch?v=xKVsupliks8

[^10]: https://www.siliconflow.com/articles/en/best-open-source-models-for-real-time-transcription

[^11]: https://openai.com/index/introducing-our-next-generation-audio-models/

[^12]: https://github.com/AudioLLMs/Awesome-Audio-LLM

[^13]: https://amical.ai/blog/open-source-transcription-software

[^14]: https://news.ycombinator.com/item?id=46731068

[^15]: https://www.meowtxt.com/blog/audio-to-text-open-source

