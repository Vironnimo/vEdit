# Offline tokenizer

vEdit uses `tiktoken-rs` 0.12.1 and its embedded `o200k_base` vocabulary to
count source text locally. No model or network service is invoked.

- `tiktoken-rs-LICENSE.txt`: https://github.com/zurawiki/tiktoken-rs/blob/main/LICENSE
- `tiktoken-LICENSE.txt`: https://github.com/openai/tiktoken/blob/main/LICENSE

The portable build copies these notices into `licenses/tokenizer/`.
