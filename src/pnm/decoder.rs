use std::io::{BufRead, BufReader, Read};

use super::{ArbitraryHeader, ArbitraryTuplType, BitmapHeader, GraymapHeader, PixmapHeader};
use super::{HeaderRecord, PNMHeader, PNMSubtype, SampleEncoding};
use color::ColorType;
use image::{DecodingResult, ImageDecoder, ImageError, ImageResult};

use byteorder::{BigEndian, ByteOrder};

/// Dynamic representation, represents all decodable (sample, depth) combinations.
#[derive(Clone, Copy)]
enum TupleType {
    PbmBit,
    BWBit,
    GrayU8,
    GrayU16,
    RGBU8,
    RGBU16,
}

trait Sample {
    type T;
    fn bytelen(width: u32, height: u32, samples: u32) -> ImageResult<usize>;
    /// It is guaranteed that `bytes.len() == bytelen(width, height, samples)`
    fn from_bytes(bytes: &[u8], width: u32, height: u32, samples: u32)
        -> ImageResult<Vec<Self::T>>;
    fn from_unsigned(u32) -> ImageResult<Self::T>;
}

struct U8;
struct U16;
struct PbmBit;
struct BWBit;

trait DecodableImageHeader {
    fn tuple_type(&self) -> ImageResult<TupleType>;
}

/// PNM decoder
pub struct PNMDecoder<R> {
    reader: BufReader<R>,
    header: PNMHeader,
    tuple: TupleType,
}

impl<R: Read> PNMDecoder<R> {
    /// Create a new decoder that decodes from the stream ```read```
    pub fn new(read: R) -> ImageResult<PNMDecoder<R>> {
        let mut buf = BufReader::new(read);
        let magic = try!(buf.read_magic_constant());
        if magic[0] != b'P' {
            return Err(ImageError::FormatError(
                "Expected magic constant for pnm, P1 through P7".to_string(),
            ));
        }

        let subtype = match magic[1] {
            b'1' => PNMSubtype::Bitmap(SampleEncoding::Ascii),
            b'2' => PNMSubtype::Graymap(SampleEncoding::Ascii),
            b'3' => PNMSubtype::Pixmap(SampleEncoding::Ascii),
            b'4' => PNMSubtype::Bitmap(SampleEncoding::Binary),
            b'5' => PNMSubtype::Graymap(SampleEncoding::Binary),
            b'6' => PNMSubtype::Pixmap(SampleEncoding::Binary),
            b'7' => PNMSubtype::ArbitraryMap,
            _ => {
                return Err(ImageError::FormatError(
                    "Expected magic constant for ppm, P1 through P7".to_string(),
                ))
            }
        };

        match subtype {
            PNMSubtype::Bitmap(enc) => PNMDecoder::read_bitmap_header(buf, enc),
            PNMSubtype::Graymap(enc) => PNMDecoder::read_graymap_header(buf, enc),
            PNMSubtype::Pixmap(enc) => PNMDecoder::read_pixmap_header(buf, enc),
            PNMSubtype::ArbitraryMap => PNMDecoder::read_arbitrary_header(buf),
        }
    }

    /// Extract the reader and header after an image has been read.
    pub fn into_inner(self) -> (R, PNMHeader) {
        (self.reader.into_inner(), self.header)
    }

    fn read_bitmap_header(
        mut reader: BufReader<R>,
        encoding: SampleEncoding,
    ) -> ImageResult<PNMDecoder<R>> {
        let header = reader.read_bitmap_header(encoding)?;
        Ok(PNMDecoder {
            reader,
            tuple: TupleType::PbmBit,
            header: PNMHeader {
                decoded: HeaderRecord::Bitmap(header),
                encoded: None,
            },
        })
    }

    fn read_graymap_header(
        mut reader: BufReader<R>,
        encoding: SampleEncoding,
    ) -> ImageResult<PNMDecoder<R>> {
        let header = reader.read_graymap_header(encoding)?;
        let tuple_type = header.tuple_type()?;
        Ok(PNMDecoder {
            reader,
            tuple: tuple_type,
            header: PNMHeader {
                decoded: HeaderRecord::Graymap(header),
                encoded: None,
            },
        })
    }

    fn read_pixmap_header(
        mut reader: BufReader<R>,
        encoding: SampleEncoding,
    ) -> ImageResult<PNMDecoder<R>> {
        let header = reader.read_pixmap_header(encoding)?;
        let tuple_type = header.tuple_type()?;
        Ok(PNMDecoder {
            reader,
            tuple: tuple_type,
            header: PNMHeader {
                decoded: HeaderRecord::Pixmap(header),
                encoded: None,
            },
        })
    }

    fn read_arbitrary_header(mut reader: BufReader<R>) -> ImageResult<PNMDecoder<R>> {
        let header = reader.read_arbitrary_header()?;
        let tuple_type = header.tuple_type()?;
        Ok(PNMDecoder {
            reader,
            tuple: tuple_type,
            header: PNMHeader {
                decoded: HeaderRecord::Arbitrary(header),
                encoded: None,
            },
        })
    }
}

trait HeaderReader: BufRead {
    /// Reads the two magic constant bytes
    fn read_magic_constant(&mut self) -> ImageResult<[u8; 2]> {
        let mut magic: [u8; 2] = [0, 0];
        self.read_exact(&mut magic)
            .map_err(|_| ImageError::NotEnoughData)?;
        Ok(magic)
    }

    /// Reads a string as well as a single whitespace after it, ignoring comments
    fn read_next_string(&mut self) -> ImageResult<String> {
        let mut bytes = Vec::new();

        // pair input bytes with a bool mask to remove comments
        let mark_comments = self.bytes().scan(true, |partof, read| {
            let byte = match read {
                Err(err) => return Some((*partof, Err(err))),
                Ok(byte) => byte,
            };
            let cur_enabled = *partof && byte != b'#';
            let next_enabled = cur_enabled || (byte == b'\r' || byte == b'\n');
            *partof = next_enabled;
            Some((cur_enabled, Ok(byte)))
        });

        for (_, byte) in mark_comments.filter(|ref e| e.0) {
            match byte {
                Ok(b'\t') | Ok(b'\n') | Ok(b'\x0b') | Ok(b'\x0c') | Ok(b'\r') | Ok(b' ') => {
                    if !bytes.is_empty() {
                        break; // We're done as we already have some content
                    }
                }
                Ok(byte) => {
                    bytes.push(byte);
                }
                Err(_) => break,
            }
        }

        if bytes.is_empty() {
            return Err(ImageError::FormatError("Unexpected eof".to_string()));
        }

        if !bytes.as_slice().is_ascii() {
            return Err(ImageError::FormatError(
                "Non ascii character in preamble".to_string(),
            ));
        }

        String::from_utf8(bytes)
            .map_err(|_| ImageError::FormatError("Couldn't read preamble".to_string()))
    }

    /// Read the next line
    fn read_next_line(&mut self) -> ImageResult<String> {
        let mut buffer = String::new();
        self.read_line(&mut buffer)
            .map_err(|_| ImageError::FormatError("Line not properly formatted".to_string()))?;
        Ok(buffer)
    }

    fn read_next_u32(&mut self) -> ImageResult<u32> {
        let s = try!(self.read_next_string());
        s.parse::<u32>()
            .map_err(|_| ImageError::FormatError("Invalid number in preamble".to_string()))
    }

    fn read_bitmap_header(&mut self, encoding: SampleEncoding) -> ImageResult<BitmapHeader> {
        let width = try!(self.read_next_u32());
        let height = try!(self.read_next_u32());
        Ok(BitmapHeader {
            encoding,
            width,
            height,
        })
    }

    fn read_graymap_header(&mut self, encoding: SampleEncoding) -> ImageResult<GraymapHeader> {
        self.read_pixmap_header(encoding).map(
            |PixmapHeader {
                 encoding,
                 width,
                 height,
                 maxval,
             }| GraymapHeader {
                encoding,
                width,
                height,
                maxwhite: maxval,
            },
        )
    }

    fn read_pixmap_header(&mut self, encoding: SampleEncoding) -> ImageResult<PixmapHeader> {
        let width = try!(self.read_next_u32());
        let height = try!(self.read_next_u32());
        let maxval = try!(self.read_next_u32());
        Ok(PixmapHeader {
            encoding,
            width,
            height,
            maxval,
        })
    }

    fn read_arbitrary_header(&mut self) -> ImageResult<ArbitraryHeader> {
        match self.bytes().next() {
            None => return Err(ImageError::FormatError("Input too short".to_string())),
            Some(Err(io)) => return Err(ImageError::IoError(io)),
            Some(Ok(b'\n')) => (),
            _ => {
                return Err(ImageError::FormatError(
                    "Expected newline after P7".to_string(),
                ))
            }
        }

        let mut line = String::new();
        let mut height: Option<u32> = None;
        let mut width: Option<u32> = None;
        let mut depth: Option<u32> = None;
        let mut maxval: Option<u32> = None;
        let mut tupltype: Option<String> = None;
        loop {
            line.truncate(0);
            self.read_line(&mut line).map_err(ImageError::IoError)?;
            if line.as_bytes()[0] == b'#' {
                continue;
            }
            if !line.is_ascii() {
                return Err(ImageError::FormatError(
                    "Only ascii characters allowed in pam header".to_string(),
                ));
            }
            let (identifier, rest) = line.trim_left()
                .split_at(line.find(char::is_whitespace).unwrap_or_else(|| line.len()));
            match identifier {
                "ENDHDR" => break,
                "HEIGHT" => if height.is_some() {
                    return Err(ImageError::FormatError("Duplicate HEIGHT line".to_string()));
                } else {
                    let h = rest.trim()
                        .parse::<u32>()
                        .map_err(|_| ImageError::FormatError("Invalid height".to_string()))?;
                    height = Some(h);
                },
                "WIDTH" => if width.is_some() {
                    return Err(ImageError::FormatError("Duplicate WIDTH line".to_string()));
                } else {
                    let w = rest.trim()
                        .parse::<u32>()
                        .map_err(|_| ImageError::FormatError("Invalid width".to_string()))?;
                    width = Some(w);
                },
                "DEPTH" => if depth.is_some() {
                    return Err(ImageError::FormatError("Duplicate DEPTH line".to_string()));
                } else {
                    let d = rest.trim()
                        .parse::<u32>()
                        .map_err(|_| ImageError::FormatError("Invalid depth".to_string()))?;
                    depth = Some(d);
                },
                "MAXVAL" => if maxval.is_some() {
                    return Err(ImageError::FormatError("Duplicate MAXVAL line".to_string()));
                } else {
                    let m = rest.trim()
                        .parse::<u32>()
                        .map_err(|_| ImageError::FormatError("Invalid maxval".to_string()))?;
                    maxval = Some(m);
                },
                "TUPLTYPE" => {
                    let identifier = rest.trim();
                    if tupltype.is_some() {
                        let appended = tupltype.take().map(|mut v| {
                            v.push(' ');
                            v.push_str(identifier);
                            v
                        });
                        tupltype = appended;
                    } else {
                        tupltype = Some(identifier.to_string());
                    }
                }
                _ => return Err(ImageError::FormatError("Unknown header line".to_string())),
            }
        }

        let (h, w, d, m) = match (height, width, depth, maxval) {
            (None, _, _, _) => {
                return Err(ImageError::FormatError(
                    "Expected one HEIGHT line".to_string(),
                ))
            }
            (_, None, _, _) => {
                return Err(ImageError::FormatError(
                    "Expected one WIDTH line".to_string(),
                ))
            }
            (_, _, None, _) => {
                return Err(ImageError::FormatError(
                    "Expected one DEPTH line".to_string(),
                ))
            }
            (_, _, _, None) => {
                return Err(ImageError::FormatError(
                    "Expected one MAXVAL line".to_string(),
                ))
            }
            (Some(h), Some(w), Some(d), Some(m)) => (h, w, d, m),
        };

        let tupltype = match tupltype {
            None => None,
            Some(ref t) if t == "BLACKANDWHITE" => Some(ArbitraryTuplType::BlackAndWhite),
            Some(ref t) if t == "BLACKANDWHITE_ALPHA" => {
                Some(ArbitraryTuplType::BlackAndWhiteAlpha)
            }
            Some(ref t) if t == "GRAYSCALE" => Some(ArbitraryTuplType::Grayscale),
            Some(ref t) if t == "GRAYSCALE_ALPHA" => Some(ArbitraryTuplType::GrayscaleAlpha),
            Some(ref t) if t == "RGB" => Some(ArbitraryTuplType::RGB),
            Some(ref t) if t == "RGB_ALPHA" => Some(ArbitraryTuplType::RGBAlpha),
            Some(other) => Some(ArbitraryTuplType::Custom(other)),
        };

        Ok(ArbitraryHeader {
            height: h,
            width: w,
            depth: d,
            maxval: m,
            tupltype,
        })
    }
}

impl<R: Read> HeaderReader for BufReader<R> {}

impl<R: Read> ImageDecoder for PNMDecoder<R> {
    fn dimensions(&mut self) -> ImageResult<(u32, u32)> {
        Ok((self.header.width(), self.header.height()))
    }

    fn colortype(&mut self) -> ImageResult<ColorType> {
        Ok(self.tuple.color())
    }

    fn row_len(&mut self) -> ImageResult<usize> {
        self.rowlen()
    }

    fn read_scanline(&mut self, _buf: &mut [u8]) -> ImageResult<u32> {
        unimplemented!();
    }

    fn read_image(&mut self) -> ImageResult<DecodingResult> {
        self.read()
    }
}

impl<R: Read> PNMDecoder<R> {
    fn rowlen(&self) -> ImageResult<usize> {
        match self.tuple {
            TupleType::PbmBit => PbmBit::bytelen(self.header.width(), 1, 1),
            TupleType::BWBit => BWBit::bytelen(self.header.width(), 1, 1),
            TupleType::RGBU8 => U8::bytelen(self.header.width(), 1, 3),
            TupleType::RGBU16 => U16::bytelen(self.header.width(), 1, 3),
            TupleType::GrayU8 => U8::bytelen(self.header.width(), 1, 1),
            TupleType::GrayU16 => U16::bytelen(self.header.width(), 1, 1),
        }
    }

    fn read(&mut self) -> ImageResult<DecodingResult> {
        match self.tuple {
            TupleType::PbmBit => self.read_samples::<PbmBit>(1),
            TupleType::BWBit => self.read_samples::<BWBit>(1),
            TupleType::RGBU8 => self.read_samples::<U8>(3),
            TupleType::RGBU16 => self.read_samples::<U16>(3),
            TupleType::GrayU8 => self.read_samples::<U8>(1),
            TupleType::GrayU16 => self.read_samples::<U16>(1),
        }
    }

    fn read_samples<S: Sample>(&mut self, components: u32) -> ImageResult<DecodingResult>
    where
        Vec<S::T>: Into<DecodingResult>,
    {
        match self.subtype().sample_encoding() {
            SampleEncoding::Binary => {
                let width = self.header.width();
                let height = self.header.height();
                let bytecount = S::bytelen(width, height, components)?;
                let mut bytes = vec![0 as u8; bytecount];
                (&mut self.reader)
                    .read_exact(&mut bytes)
                    .map_err(|_| ImageError::NotEnoughData)?;
                let samples = S::from_bytes(&bytes, width, height, components)?;
                Ok(samples.into())
            }
            SampleEncoding::Ascii => {
                let samples = self.read_ascii::<S>(components)?;
                Ok(samples.into())
            }
        }
    }

    fn read_ascii<Basic: Sample>(&mut self, components: u32) -> ImageResult<Vec<Basic::T>> {
        let mut buffer = Vec::new();
        for _ in 0..(self.header.width() * self.header.height() * components) {
            let value = self.read_ascii_sample()?;
            let sample = Basic::from_unsigned(value)?;
            buffer.push(sample);
        }
        Ok(buffer)
    }

    fn read_ascii_sample(&mut self) -> ImageResult<u32> {
        let istoken = |v: &Result<u8, _>| match *v {
            Err(_) => false,
            Ok(b'\t') | Ok(b'\n') | Ok(b'\x0b') | Ok(b'\x0c') | Ok(b'\r') | Ok(b' ') => false,
            _ => true,
        };
        let token = (&mut self.reader)
            .bytes()
            .skip_while(|v| !istoken(v))
            .take_while(&istoken)
            .collect::<Result<Vec<u8>, _>>()?;
        if !token.is_ascii() {
            return Err(ImageError::FormatError(
                "Non ascii character where sample value was expected".to_string(),
            ));
        }
        let string = String::from_utf8(token)
            .map_err(|_| ImageError::FormatError("Error parsing sample".to_string()))?;
        string
            .parse::<u32>()
            .map_err(|_| ImageError::FormatError("Error parsing sample value".to_string()))
    }

    /// Get the pnm subtype, depending on the magic constant contained in the header
    pub fn subtype(&self) -> PNMSubtype {
        self.header.subtype()
    }
}

impl TupleType {
    fn color(self) -> ColorType {
        use self::TupleType::*;
        match self {
            PbmBit => ColorType::Gray(1),
            BWBit => ColorType::Gray(1),
            GrayU8 => ColorType::Gray(8),
            GrayU16 => ColorType::Gray(16),
            RGBU8 => ColorType::RGB(8),
            RGBU16 => ColorType::GrayA(16),
        }
    }
}

impl Sample for U8 {
    type T = u8;

    fn bytelen(width: u32, height: u32, samples: u32) -> ImageResult<usize> {
        Ok((width * height * samples) as usize)
    }

    fn from_bytes(
        bytes: &[u8],
        _width: u32,
        _height: u32,
        _samples: u32,
    ) -> ImageResult<Vec<Self::T>> {
        let mut buffer = Vec::new();
        buffer.resize(bytes.len(), 0 as u8);
        buffer.copy_from_slice(bytes);
        Ok(buffer)
    }

    fn from_unsigned(val: u32) -> ImageResult<Self::T> {
        if val > u32::from(u8::max_value()) {
            Err(ImageError::FormatError(
                "Sample value outside of bounds".to_string(),
            ))
        } else {
            Ok(val as u8)
        }
    }
}

impl Sample for U16 {
    type T = u16;

    fn bytelen(width: u32, height: u32, samples: u32) -> ImageResult<usize> {
        Ok((width * height * samples * 2) as usize)
    }

    fn from_bytes(
        bytes: &[u8],
        width: u32,
        height: u32,
        samples: u32,
    ) -> ImageResult<Vec<Self::T>> {
        let mut buffer = Vec::new();
        buffer.resize((width * height * samples) as usize, 0 as u16);
        BigEndian::read_u16_into(bytes, &mut buffer);
        Ok(buffer)
    }

    fn from_unsigned(val: u32) -> ImageResult<Self::T> {
        if val > u32::from(u16::max_value()) {
            Err(ImageError::FormatError(
                "Sample value outside of bounds".to_string(),
            ))
        } else {
            Ok(val as u16)
        }
    }
}

// The image is encoded in rows of bits, high order bits first. Any bits beyond the row bits should
// be ignored. Also, contrary to rgb, black pixels are encoded as a 1 while white is 0. This will
// need to be reversed for the grayscale output.
impl Sample for PbmBit {
    type T = u8;

    fn bytelen(width: u32, height: u32, samples: u32) -> ImageResult<usize> {
        let count = width * samples;
        let linelen = (count / 8) + ((count % 8) != 0) as u32;
        Ok((linelen * height) as usize)
    }

    fn from_bytes(
        bytes: &[u8],
        width: u32,
        height: u32,
        samples: u32,
    ) -> ImageResult<Vec<Self::T>> {
        let mut buffer = Vec::new();
        let linecount = width * samples;
        let linebytelen = (linecount / 8) + ((linecount % 8) != 0) as u32;
        buffer.resize((width * height * samples) as usize, 0 as u8);
        for (line, linebuffer) in bytes.chunks(linebytelen as usize).enumerate() {
            let outbase = line * linecount as usize;
            for samplei in 0..linecount {
                let byteindex = (samplei / 8) as usize;
                let inindex = 7 - samplei % 8;
                let indicator = (linebuffer[byteindex] >> inindex) & 0x01;
                buffer[outbase + samplei as usize] = if indicator == 0 { 1 } else { 0 };
            }
        }
        Ok(buffer)
    }

    fn from_unsigned(val: u32) -> ImageResult<Self::T> {
        match val {
            // 0 is white in pbm
            0 => Ok(1 as u8),
            // 1 is black in pbm
            1 => Ok(0 as u8),
            _ => Err(ImageError::FormatError(
                "Sample value outside of bounds".to_string(),
            )),
        }
    }
}

// Encoded just like a normal U8 but we check the values.
impl Sample for BWBit {
    type T = u8;

    fn bytelen(width: u32, height: u32, samples: u32) -> ImageResult<usize> {
        U8::bytelen(width, height, samples)
    }

    fn from_bytes(
        bytes: &[u8],
        width: u32,
        height: u32,
        samples: u32,
    ) -> ImageResult<Vec<Self::T>> {
        let values = U8::from_bytes(bytes, width, height, samples)?;
        if values.iter().any(|&val| val > 1) {
            return Err(ImageError::FormatError(
                "Sample value outside of bounds".to_string(),
            ));
        };
        Ok(values)
    }

    fn from_unsigned(val: u32) -> ImageResult<Self::T> {
        match val {
            0 => Ok(0 as u8),
            1 => Ok(1 as u8),
            _ => Err(ImageError::FormatError(
                "Sample value outside of bounds".to_string(),
            )),
        }
    }
}

impl Into<DecodingResult> for Vec<u8> {
    fn into(self) -> DecodingResult {
        DecodingResult::U8(self)
    }
}

impl Into<DecodingResult> for Vec<u16> {
    fn into(self) -> DecodingResult {
        DecodingResult::U16(self)
    }
}

impl DecodableImageHeader for BitmapHeader {
    fn tuple_type(&self) -> ImageResult<TupleType> {
        Ok(TupleType::PbmBit)
    }
}

impl DecodableImageHeader for GraymapHeader {
    fn tuple_type(&self) -> ImageResult<TupleType> {
        match self.maxwhite {
            v if v <= 0xFF => Ok(TupleType::GrayU8),
            v if v <= 0xFFFF => Ok(TupleType::GrayU16),
            _ => Err(ImageError::FormatError(
                "Image maxval is not less or equal to 65535".to_string(),
            )),
        }
    }
}

impl DecodableImageHeader for PixmapHeader {
    fn tuple_type(&self) -> ImageResult<TupleType> {
        match self.maxval {
            v if v <= 0xFF => Ok(TupleType::RGBU8),
            v if v <= 0xFFFF => Ok(TupleType::RGBU16),
            _ => Err(ImageError::FormatError(
                "Image maxval is not less or equal to 65535".to_string(),
            )),
        }
    }
}

impl DecodableImageHeader for ArbitraryHeader {
    fn tuple_type(&self) -> ImageResult<TupleType> {
        match self.tupltype {
            None if self.depth == 1 => Ok(TupleType::GrayU8),
            None if self.depth == 2 => Err(ImageError::UnsupportedColor(ColorType::GrayA(8))),
            None if self.depth == 3 => Ok(TupleType::RGBU8),
            None if self.depth == 4 => Err(ImageError::UnsupportedColor(ColorType::RGBA(8))),

            Some(ArbitraryTuplType::BlackAndWhite) if self.maxval == 1 && self.depth == 1 => {
                Ok(TupleType::BWBit)
            }
            Some(ArbitraryTuplType::BlackAndWhite) => Err(ImageError::FormatError(
                "Invalid depth or maxval for tuple type BLACKANDWHITE".to_string(),
            )),

            Some(ArbitraryTuplType::Grayscale) if self.depth == 1 && self.maxval <= 0xFF => {
                Ok(TupleType::GrayU8)
            }
            Some(ArbitraryTuplType::Grayscale) if self.depth <= 1 && self.maxval <= 0xFFFF => {
                Ok(TupleType::GrayU16)
            }
            Some(ArbitraryTuplType::Grayscale) => Err(ImageError::FormatError(
                "Invalid depth or maxval for tuple type GRAYSCALE".to_string(),
            )),

            Some(ArbitraryTuplType::RGB) if self.depth == 3 && self.maxval <= 0xFF => {
                Ok(TupleType::RGBU8)
            }
            Some(ArbitraryTuplType::RGB) if self.depth == 3 && self.maxval <= 0xFFFF => {
                Ok(TupleType::RGBU16)
            }
            Some(ArbitraryTuplType::RGB) => Err(ImageError::FormatError(
                "Invalid depth for tuple type RGB".to_string(),
            )),

            Some(ArbitraryTuplType::BlackAndWhiteAlpha) => {
                Err(ImageError::UnsupportedColor(ColorType::GrayA(1)))
            }
            Some(ArbitraryTuplType::GrayscaleAlpha) => {
                Err(ImageError::UnsupportedColor(ColorType::GrayA(8)))
            }
            Some(ArbitraryTuplType::RGBAlpha) => {
                Err(ImageError::UnsupportedColor(ColorType::RGBA(8)))
            }
            _ => Err(ImageError::FormatError(
                "Tuple type not recognized".to_string(),
            )),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Tests reading of a valid blackandwhite pam
    #[test]
    fn pam_blackandwhite() {
        let pamdata = b"P7
WIDTH 4
HEIGHT 4
DEPTH 1
MAXVAL 1
TUPLTYPE BLACKANDWHITE
# Comment line
ENDHDR
\x01\x00\x00\x01\x01\x00\x00\x01\x01\x00\x00\x01\x01\x00\x00\x01";
        let mut decoder = PNMDecoder::new(&pamdata[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(1));
        assert_eq!(decoder.dimensions().unwrap(), (4, 4));
        assert_eq!(decoder.subtype(), PNMSubtype::ArbitraryMap);
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(
                data,
                vec![
                    0x01, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x01,
                    0x00, 0x00, 0x01,
                ]
            ),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Arbitrary(ArbitraryHeader {
                            width: 4,
                            height: 4,
                            maxval: 1,
                            depth: 1,
                            tupltype: Some(ArbitraryTuplType::BlackAndWhite),
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    /// Tests reading of a valid grayscale pam
    #[test]
    fn pam_grayscale() {
        let pamdata = b"P7
WIDTH 4
HEIGHT 4
DEPTH 1
MAXVAL 255
TUPLTYPE GRAYSCALE
# Comment line
ENDHDR
\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef";
        let mut decoder = PNMDecoder::new(&pamdata[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(8));
        assert_eq!(decoder.dimensions().unwrap(), (4, 4));
        assert_eq!(decoder.subtype(), PNMSubtype::ArbitraryMap);
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(
                data,
                vec![
                    0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde,
                    0xad, 0xbe, 0xef,
                ]
            ),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Arbitrary(ArbitraryHeader {
                            width: 4,
                            height: 4,
                            depth: 1,
                            maxval: 255,
                            tupltype: Some(ArbitraryTuplType::Grayscale),
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    /// Tests reading of a valid rgb pam
    #[test]
    fn pam_rgb() {
        let pamdata = b"P7
# Comment line
MAXVAL 255
TUPLTYPE RGB
DEPTH 3
WIDTH 2
HEIGHT 2
ENDHDR
\xde\xad\xbe\xef\xde\xad\xbe\xef\xde\xad\xbe\xef";
        let mut decoder = PNMDecoder::new(&pamdata[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::RGB(8));
        assert_eq!(decoder.dimensions().unwrap(), (2, 2));
        assert_eq!(decoder.subtype(), PNMSubtype::ArbitraryMap);
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(
                data,
                vec![
                    0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef,
                ]
            ),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Arbitrary(ArbitraryHeader {
                            maxval: 255,
                            tupltype: Some(ArbitraryTuplType::RGB),
                            depth: 3,
                            width: 2,
                            height: 2,
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    #[test]
    fn pbm_binary() {
        // The data contains two rows of the image (each line is padded to the full byte). For
        // comments on its format, see documentation of `impl SampleType for PbmBit`.
        let pbmbinary = [&b"P4 6 2\n"[..], &[0b01101100 as u8, 0b10110111]].concat();
        let mut decoder = PNMDecoder::new(&pbmbinary[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(1));
        assert_eq!(decoder.dimensions().unwrap(), (6, 2));
        assert_eq!(
            decoder.subtype(),
            PNMSubtype::Bitmap(SampleEncoding::Binary)
        );
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(data, vec![1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Bitmap(BitmapHeader {
                            encoding: SampleEncoding::Binary,
                            width: 6,
                            height: 2,
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    #[test]
    fn pbm_ascii() {
        // The data contains two rows of the image (each line is padded to the full byte). For
        // comments on its format, see documentation of `impl SampleType for PbmBit`.
        let pbmbinary = b"P1 6 2\n 0 1 1 0 1 1\n1 0 1 1 0 1";
        let mut decoder = PNMDecoder::new(&pbmbinary[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(1));
        assert_eq!(decoder.dimensions().unwrap(), (6, 2));
        assert_eq!(decoder.subtype(), PNMSubtype::Bitmap(SampleEncoding::Ascii));
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(data, vec![1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Bitmap(BitmapHeader {
                            encoding: SampleEncoding::Ascii,
                            width: 6,
                            height: 2,
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    #[test]
    fn pgm_binary() {
        // The data contains two rows of the image (each line is padded to the full byte). For
        // comments on its format, see documentation of `impl SampleType for PbmBit`.
        let elements = (0..16).collect::<Vec<_>>();
        let pbmbinary = [&b"P5 4 4 255\n"[..], &elements].concat();
        let mut decoder = PNMDecoder::new(&pbmbinary[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(8));
        assert_eq!(decoder.dimensions().unwrap(), (4, 4));
        assert_eq!(
            decoder.subtype(),
            PNMSubtype::Graymap(SampleEncoding::Binary)
        );
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(data, elements),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Graymap(GraymapHeader {
                            encoding: SampleEncoding::Binary,
                            width: 4,
                            height: 4,
                            maxwhite: 255,
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }

    #[test]
    fn pgm_ascii() {
        // The data contains two rows of the image (each line is padded to the full byte). For
        // comments on its format, see documentation of `impl SampleType for PbmBit`.
        let pbmbinary = b"P2 4 4 255\n 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15";
        let mut decoder = PNMDecoder::new(&pbmbinary[..]).unwrap();
        assert_eq!(decoder.colortype().unwrap(), ColorType::Gray(8));
        assert_eq!(decoder.dimensions().unwrap(), (4, 4));
        assert_eq!(
            decoder.subtype(),
            PNMSubtype::Graymap(SampleEncoding::Ascii)
        );
        match decoder.read_image().unwrap() {
            DecodingResult::U16(_) => panic!("Decoded wrong image format"),
            DecodingResult::U8(data) => assert_eq!(data, (0..16).collect::<Vec<_>>()),
        }
        match decoder.into_inner() {
            (
                _,
                PNMHeader {
                    decoded:
                        HeaderRecord::Graymap(GraymapHeader {
                            encoding: SampleEncoding::Ascii,
                            width: 4,
                            height: 4,
                            maxwhite: 255,
                        }),
                    encoded: _,
                },
            ) => (),
            _ => panic!("Decoded header is incorrect"),
        }
    }
}
