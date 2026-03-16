use std::{
    io::{self, Write},
    mem,
};

use ffi::mfxBitstream;
use intel_onevpl_sys as ffi;

use crate::constants::{BitstreamDataFlags, Codec, FrameType, PicStruct};

#[derive(Debug)]
enum BitstreamStorage<'a> {
    Borrowed(&'a mut [u8]),
    Owned(Vec<u8>),
}

impl BitstreamStorage<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Borrowed(buffer) => buffer.len(),
            Self::Owned(buffer) => buffer.len(),
        }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Borrowed(buffer) => buffer,
            Self::Owned(buffer) => buffer,
        }
    }
}

#[derive(Debug)]
pub struct Bitstream<'a> {
    storage: BitstreamStorage<'a>,
    pub(crate) inner: mfxBitstream,
}
unsafe impl Send for Bitstream<'_> {}

impl<'a> Bitstream<'a> {
    /// Creates a data source/destination for encoded/decoded/processed data
    #[tracing::instrument]
    pub fn with_codec(buffer: &'a mut [u8], codec: Codec) -> Self {
        Self::new_with_storage(BitstreamStorage::Borrowed(buffer), codec)
    }

    fn new_with_storage(storage: BitstreamStorage<'a>, codec: Codec) -> Self {
        let mut storage = storage;
        let mut bitstream: mfxBitstream = unsafe { mem::zeroed() };
        let (data_ptr, max_len) = match &mut storage {
            BitstreamStorage::Borrowed(buffer) => (buffer.as_mut_ptr(), buffer.len()),
            BitstreamStorage::Owned(buffer) => (buffer.as_mut_ptr(), buffer.len()),
        };
        bitstream.Data = data_ptr;
        bitstream.MaxLength = max_len as u32;
        bitstream.__bindgen_anon_1.__bindgen_anon_1.CodecId = codec as u32;
        Self {
            storage,
            inner: bitstream,
        }
    }

    pub fn codec(&self) -> Codec {
        Codec::from_repr(
            unsafe { self.inner.__bindgen_anon_1.__bindgen_anon_1.CodecId } as ffi::_bindgen_ty_14,
        )
        .unwrap()
    }

    /// The size of the backing buffer
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// The amount of data currently in the bitstream
    pub fn size(&self) -> u32 {
        self.inner.DataLength
    }

    /// Reading or writing offset in the buffer
    pub fn offset(&self) -> u32 {
        self.inner.DataOffset
    }

    /// Set the amount of data currently in the bitstream. Useful for when you
    /// add a buffer to a bitstream that already contains data.
    pub fn set_size(&mut self, size: usize) {
        assert!(size <= self.inner.MaxLength as usize);
        self.inner.DataLength = size as u32;
    }

    pub fn set_flags(&mut self, flags: BitstreamDataFlags) {
        self.inner.DataFlag = flags.bits();
    }

    #[doc = " The FrameType enumerator itemizes frame types. Use bit-ORed values to specify all that apply."]
    pub fn frame_type(&self) -> FrameType {
        FrameType::from_bits(self.inner.FrameType as ffi::_bindgen_ty_37).unwrap()
    }

    pub fn pic_struct(&self) -> PicStruct {
        PicStruct::from_repr(self.inner.PicStruct as ffi::_bindgen_ty_6).unwrap()
    }

    pub fn timestamp(&self) -> u64 {
        self.inner.TimeStamp
    }

    pub fn decode_timestamp(&self) -> i64 {
        self.inner.DecodeTimeStamp
    }
}

impl Bitstream<'static> {
    /// Creates a bitstream backed by an owned buffer.
    pub fn with_owned_capacity(capacity: usize, codec: Codec) -> Self {
        Self::new_with_storage(BitstreamStorage::Owned(vec![0_u8; capacity]), codec)
    }
}

impl io::Write for Bitstream<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let data_offset = self.inner.DataOffset as usize;
        let data_len = self.inner.DataLength as usize;
        let buffer_len = self.storage.len();

        if data_len >= buffer_len {
            return Ok(0);
        }

        {
            let buffer = self.storage.as_mut_slice();
            if data_offset > 0 {
                // Move all data after DataOffset to the beginning of Data
                let data_end = data_offset + data_len;
                buffer.copy_within(data_offset..data_end, 0);
                self.inner.DataOffset = 0;
            }

            let free_buffer_len = buffer.len() - data_len;
            let copy_len = usize::min(free_buffer_len, buf.len());
            buffer[data_len..data_len + copy_len].copy_from_slice(&buf[..copy_len]);
            self.inner.DataLength += copy_len as u32;
            Ok(copy_len)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Read for Bitstream<'_> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let data_len = self.inner.DataLength as usize;
        let bytes = {
            let buffer = self.storage.as_mut_slice();
            let bytes = buf.write(&buffer[..data_len])?;
            buffer.copy_within(bytes..data_len, 0);
            bytes
        };
        self.inner.DataLength -= bytes as u32;

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use rand::Fill;
    use std::io::Read;

    use super::Bitstream;

    #[test]
    fn bitstream_read_write() {
        let mut rng = rand::thread_rng();
        let mut input_data = vec![0u8; 8192];
        let input_data_len = input_data.len();
        input_data[..].try_fill(&mut rng).unwrap();
        let copy_input_data = input_data.clone();

        let mut bitstream = Bitstream::with_codec(&mut input_data, crate::constants::Codec::AVC);

        bitstream.set_size(input_data_len);
        assert_eq!(bitstream.size() as usize, input_data_len);

        let mut bytes_read = 0;
        while bitstream.size() > 0 {
            let mut buffer = vec![0u8; 1000];
            let bytes = bitstream.read(&mut buffer).unwrap();

            assert_eq!(
                copy_input_data[bytes_read..bytes_read + bytes],
                buffer[..bytes]
            );

            bytes_read += bytes;
        }

        assert_eq!(bytes_read, copy_input_data.len());
    }

    #[test]
    fn owned_bitstream_has_requested_capacity() {
        let bitstream = Bitstream::with_owned_capacity(4096, crate::constants::Codec::HEVC);
        assert_eq!(bitstream.len(), 4096);
        assert_eq!(bitstream.size(), 0);
    }
}
