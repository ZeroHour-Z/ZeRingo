//! 网易云音乐 NCM 文件解密。
//!
//! 文件结构：魔数 "CTENFDAM" | 2 字节 | RC4 密钥块 | 元数据块 | CRC + 5 字节 |
//! 封面块 | 加密音频。音频本身是 MP3 或 FLAC，解密后原样可用。

use aes::Aes128;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};

const MAGIC: &[u8; 8] = b"CTENFDAM";
const CORE_KEY: &[u8; 16] = b"hzHRAmso5kInbaxW";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerFormat {
    Mp3,
    Flac,
}

pub struct Decrypted {
    pub format: InnerFormat,
    pub audio: Vec<u8>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or("NCM 文件已损坏或被截断")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
}

/// AES-128-ECB 解密并去掉 PKCS#7 填充。
fn aes_ecb_decrypt(key: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err("密钥块长度不是 16 的倍数".into());
    }
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut out = data.to_vec();
    for block in out.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    let pad = *out.last().unwrap() as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return Err("密钥块填充无效".into());
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

fn build_key_box(key: &[u8]) -> [u8; 256] {
    let mut bx = [0u8; 256];
    for (i, b) in bx.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut last: u8 = 0;
    for i in 0..256 {
        let swap = bx[i];
        let c = swap.wrapping_add(last).wrapping_add(key[i % key.len()]);
        bx[i] = bx[c as usize];
        bx[c as usize] = swap;
        last = c;
    }
    bx
}

/// 音频流的密钥流：第 i 个字节（从音频开头算起）与此值异或。
fn stream_byte(bx: &[u8; 256], i: usize) -> u8 {
    let j = (i + 1) & 0xff;
    let a = bx[j] as usize;
    bx[(a + bx[(a + j) & 0xff] as usize) & 0xff]
}

fn detect(head: &[u8]) -> Option<InnerFormat> {
    match head {
        [b'f', b'L', b'a', b'C', ..] => Some(InnerFormat::Flac),
        [b'I', b'D', b'3', ..] => Some(InnerFormat::Mp3),
        [0xFF, b, ..] if b & 0xE0 == 0xE0 => Some(InnerFormat::Mp3),
        _ => None,
    }
}

fn decrypt_head(bx: &[u8; 256], data: &[u8]) -> Vec<u8> {
    data.iter().take(4).enumerate().map(|(i, b)| b ^ stream_byte(bx, i)).collect()
}

pub fn decrypt(data: &[u8]) -> Result<Decrypted, String> {
    if data.len() < 10 || &data[..8] != MAGIC {
        return Err("不是有效的 NCM 文件".into());
    }
    let mut r = Reader { data, pos: 10 };

    // RC4 密钥
    let key_len = r.u32()?;
    let key_enc: Vec<u8> = r.take(key_len)?.iter().map(|b| b ^ 0x64).collect();
    let key = aes_ecb_decrypt(CORE_KEY, &key_enc)?;
    let key = key.get(17..).filter(|k| !k.is_empty()).ok_or("密钥内容无效")?;
    let bx = build_key_box(key);

    // 元数据（歌名、格式等），这里不需要，直接跳过
    let meta_len = r.u32()?;
    r.take(meta_len)?;
    r.take(5)?; // CRC32 + 1 字节版本号
    // 封面区：frame_len(u32) image_len(u32) image[image_len] 填充[frame_len - image_len]
    // 没有封面时 image_len 为 0，但仍预留 frame_len 字节。
    let frame_len = r.u32()?;
    let image_len = r.u32()?;
    let after = r.pos;
    // 首选按 frame_len 跳；再按 image_len 跳（部分旧文件），取解密后开头是合法 MP3/FLAC 的那个。
    let candidates = [after + frame_len.max(image_len), after + image_len, after + frame_len];
    for start in candidates {
        if start < data.len() {
            if let Some(format) = detect(&decrypt_head(&bx, &data[start..])) {
                let audio = data[start..]
                    .iter()
                    .enumerate()
                    .map(|(i, b)| b ^ stream_byte(&bx, i))
                    .collect();
                return Ok(Decrypted { format, audio });
            }
        }
    }
    Err("无法识别解密后的音频格式".into())
}

/// 测试用：把 MP3/FLAC 数据封装成 NCM（新版封面布局）。
#[cfg(test)]
pub fn encrypt_for_test(audio: &[u8], key: &[u8]) -> Vec<u8> {
    encrypt_for_test_with(audio, key, &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3], 12)
}

/// 测试用：可指定封面数据和封面区总长度（frame_len）。
#[cfg(test)]
pub fn encrypt_for_test_with(audio: &[u8], key: &[u8], image: &[u8], frame_len: usize) -> Vec<u8> {
    use aes::cipher::BlockEncrypt;
    let mut plain = b"neteasecloudmusic".to_vec();
    plain.extend_from_slice(key);
    let pad = 16 - plain.len() % 16;
    plain.extend(std::iter::repeat_n(pad as u8, pad));
    let cipher = Aes128::new(GenericArray::from_slice(CORE_KEY));
    for block in plain.chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    let key_block: Vec<u8> = plain.iter().map(|b| b ^ 0x64).collect();

    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(key_block.len() as u32).to_le_bytes());
    out.extend_from_slice(&key_block);
    out.extend_from_slice(&0u32.to_le_bytes()); // 无元数据
    out.extend_from_slice(&[0, 0, 0, 0, 1]); // CRC32 + 版本号
    out.extend_from_slice(&(frame_len as u32).to_le_bytes());
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    out.extend_from_slice(image);
    out.extend(std::iter::repeat_n(0u8, frame_len - image.len()));
    let bx = build_key_box(key);
    out.extend(audio.iter().enumerate().map(|(i, b)| b ^ stream_byte(&bx, i)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_less_file_with_reserved_frame() {
        // 无封面（image_len = 0）但预留了封面区，这是之前失败的情况
        let audio = b"fLaC\0\0\0\x22rest-of-flac";
        let ncm = encrypt_for_test_with(audio, b"key-for-cover-less", &[], 7570);
        let d = decrypt(&ncm).unwrap();
        assert_eq!(d.format, InnerFormat::Flac);
        assert_eq!(d.audio, audio);
    }
}

#[cfg(test)]
mod dir_check {
    /// 只读检查：NCM_TEST_DIR=目录 cargo test -- --ignored check_dir --nocapture
    #[test]
    #[ignore]
    fn check_dir() {
        let dir = std::env::var("NCM_TEST_DIR").unwrap();
        let (mut ok, mut bad) = (0, 0);
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "ncm") {
                match super::decrypt(&std::fs::read(&p).unwrap()) {
                    Ok(d) => { ok += 1; println!("OK  {:?} {}", d.format, p.display()) }
                    Err(e) => { bad += 1; println!("BAD {e} {}", p.display()) }
                }
            }
        }
        println!("ok={ok} bad={bad}");
        assert_eq!(bad, 0);
    }
}
