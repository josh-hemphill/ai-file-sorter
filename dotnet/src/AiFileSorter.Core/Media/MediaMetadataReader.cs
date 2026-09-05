using System.Text;
using AiFileSorter.Core.IO;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Media;

/// <summary>Reads audio/video tags with shared file access so other apps can keep the file open.</summary>
public sealed class MediaMetadataReader
{
    private const int MaxTagBytes = 2 * 1024 * 1024;

    public MediaMetadata? Read(string path)
    {
        var extension = Path.GetExtension(path).ToLowerInvariant();
        var bytes = LockTolerantFileAccess.TryReadAll(path, MaxTagBytes, out var lockReason);
        if (bytes is null)
        {
            return lockReason is null ? null : new MediaMetadata { Source = "locked:" + lockReason };
        }

        return extension switch
        {
            ".mp3" => ParseId3(bytes) ?? ParseId3v1(bytes),
            ".flac" => ParseFlac(bytes),
            ".ogg" or ".oga" or ".opus" => ParseOggVorbis(bytes),
            ".m4a" or ".mp4" or ".m4v" or ".mov" or ".3gp" => ParseMp4(bytes),
            _ => ParseId3(bytes) ?? ParseId3v1(bytes)
        };
    }

    public static MediaMetadata? ParseId3(byte[] bytes)
    {
        if (bytes.Length < 10 || bytes[0] != (byte)'I' || bytes[1] != (byte)'D' || bytes[2] != (byte)'3')
        {
            return null;
        }

        var version = bytes[3];
        var flags = bytes[5];
        var tagSize = ReadSynchsafe(bytes, 6);
        var headerSize = 10;
        if ((flags & 0x40) != 0 && bytes.Length >= 14)
        {
            headerSize += 4 + (int)ReadSynchsafe(bytes, 10);
        }

        var available = Math.Min(bytes.Length - headerSize, (int)tagSize);
        if (available <= 0)
        {
            return null;
        }

        var body = bytes.AsSpan(headerSize, available).ToArray();
        if ((flags & 0x80) != 0)
        {
            body = RemoveUnsynchronization(body);
        }

        var metadata = new MutableMetadata { Source = "id3v2" };
        var offset = 0;
        while (offset + 10 <= body.Length)
        {
            if (body[offset] == 0)
            {
                break;
            }

            var frameId = Encoding.ASCII.GetString(body, offset, 4);
            int frameSize;
            int frameHeader = 10;
            if (version == 2)
            {
                if (offset + 6 > body.Length)
                {
                    break;
                }

                frameId = Encoding.ASCII.GetString(body, offset, 3);
                frameSize = (body[offset + 3] << 16) | (body[offset + 4] << 8) | body[offset + 5];
                frameHeader = 6;
            }
            else if (version >= 4)
            {
                frameSize = (int)ReadSynchsafe(body, offset + 4);
            }
            else
            {
                frameSize = (int)ReadU32Be(body, offset + 4);
            }

            offset += frameHeader;
            if (frameSize <= 0 || offset + frameSize > body.Length)
            {
                break;
            }

            var decoded = DecodeId3Text(body.AsSpan(offset, frameSize));
            offset += frameSize;
            if (string.IsNullOrWhiteSpace(decoded))
            {
                continue;
            }

            switch (frameId)
            {
                case "TIT2" or "TT2":
                    metadata.Title ??= decoded;
                    break;
                case "TPE1" or "TP1":
                    metadata.Artist ??= decoded;
                    break;
                case "TALB" or "TAL":
                    metadata.Album ??= decoded;
                    break;
                case "TYER" or "TYE" or "TDRC":
                    metadata.Year ??= decoded;
                    break;
                case "TCON" or "TCO":
                    metadata.Genre ??= decoded;
                    break;
                case "TLEN":
                    if (int.TryParse(decoded, out var millis) && millis > 0)
                    {
                        metadata.DurationSeconds ??= millis / 1000;
                    }
                    break;
            }
        }

        return metadata.ToMetadata();
    }

    public static MediaMetadata? ParseId3v1(byte[] bytes)
    {
        if (bytes.Length < 128)
        {
            return null;
        }

        var tag = bytes.AsSpan(bytes.Length - 128);
        if (tag[0] != (byte)'T' || tag[1] != (byte)'A' || tag[2] != (byte)'G')
        {
            return null;
        }

        return new MediaMetadata
        {
            Title = TrimLatin1(tag.Slice(3, 30)),
            Artist = TrimLatin1(tag.Slice(33, 30)),
            Album = TrimLatin1(tag.Slice(63, 30)),
            Year = TrimLatin1(tag.Slice(93, 4)),
            Genre = null,
            Source = "id3v1"
        };
    }

    public static MediaMetadata? ParseFlac(byte[] bytes)
    {
        if (bytes.Length < 8 || Encoding.ASCII.GetString(bytes, 0, 4) != "fLaC")
        {
            return null;
        }

        var offset = 4;
        while (offset + 4 <= bytes.Length)
        {
            var header = bytes[offset];
            var isLast = (header & 0x80) != 0;
            var blockType = header & 0x7F;
            var size = (bytes[offset + 1] << 16) | (bytes[offset + 2] << 8) | bytes[offset + 3];
            offset += 4;
            if (offset + size > bytes.Length)
            {
                break;
            }

            if (blockType == 4)
            {
                var metadata = ParseVorbisComment(bytes.AsSpan(offset, size));
                if (metadata is not null)
                {
                    return metadata with { Source = "flac" };
                }
            }

            offset += size;
            if (isLast)
            {
                break;
            }
        }

        return null;
    }

    public static MediaMetadata? ParseOggVorbis(byte[] bytes)
    {
        var marker = "vorbis"u8;
        for (var i = 0; i < bytes.Length - marker.Length - 8; i++)
        {
            if (bytes[i] == 3 && bytes.AsSpan(i + 1, marker.Length).SequenceEqual(marker))
            {
                var payload = bytes.AsSpan(i + 1 + marker.Length);
                var metadata = ParseVorbisComment(payload);
                return metadata is null ? null : metadata with { Source = "ogg" };
            }
        }

        return null;
    }

    public static MediaMetadata? ParseMp4(byte[] bytes)
    {
        var metadata = new MutableMetadata { Source = "mp4" };
        WalkAtoms(bytes, 0, bytes.Length, metadata, depth: 0);
        return metadata.ToMetadata();
    }

    private static void WalkAtoms(byte[] bytes, int start, int end, MutableMetadata metadata, int depth)
    {
        if (depth > 12)
        {
            return;
        }

        var offset = start;
        while (offset + 8 <= end)
        {
            var size = (int)ReadU32Be(bytes, offset);
            if (size == 0)
            {
                size = end - offset;
            }

            if (size < 8 || offset + size > end)
            {
                break;
            }

            var type = Encoding.ASCII.GetString(bytes, offset + 4, 4);
            var payloadStart = offset + 8;
            var payloadEnd = offset + size;

            if (type is "moov" or "udta" or "meta" or "ilst")
            {
                if (type == "meta" && payloadStart + 4 <= payloadEnd)
                {
                    payloadStart += 4;
                }

                WalkAtoms(bytes, payloadStart, payloadEnd, metadata, depth + 1);
            }
            else if (type is "©nam" or "©ART" or "©alb" or "©day" or "©gen" or "gnre" or "name" or "trkn")
            {
                var text = ReadMp4Text(bytes, payloadStart, payloadEnd);
                switch (type)
                {
                    case "©nam" or "name":
                        metadata.Title ??= text;
                        break;
                    case "©ART":
                        metadata.Artist ??= text;
                        break;
                    case "©alb":
                        metadata.Album ??= text;
                        break;
                    case "©day":
                        metadata.Year ??= text;
                        break;
                    case "©gen" or "gnre":
                        metadata.Genre ??= text;
                        break;
                }
            }
            else if (type is "data" && depth > 0)
            {
                var text = ReadMp4DataText(bytes, payloadStart, payloadEnd);
                metadata.Title ??= text;
            }

            offset += size;
        }
    }

    private static string? ReadMp4Text(byte[] bytes, int start, int end)
    {
        var offset = start;
        while (offset + 8 <= end)
        {
            var size = (int)ReadU32Be(bytes, offset);
            if (size < 8 || offset + size > end)
            {
                break;
            }

            var type = Encoding.ASCII.GetString(bytes, offset + 4, 4);
            if (type == "data")
            {
                return ReadMp4DataText(bytes, offset + 8, offset + size);
            }

            offset += size;
        }

        return Sanitize(Encoding.UTF8.GetString(bytes, start, Math.Max(0, end - start)));
    }

    private static string? ReadMp4DataText(byte[] bytes, int start, int end)
    {
        if (end - start < 8)
        {
            return null;
        }

        return Sanitize(Encoding.UTF8.GetString(bytes, start + 8, end - start - 8));
    }

    private static MediaMetadata? ParseVorbisComment(ReadOnlySpan<byte> payload)
    {
        if (payload.Length < 8)
        {
            return null;
        }

        var offset = 0;
        if (!TryReadU32Le(payload, ref offset, out var vendorLength) || offset + vendorLength > payload.Length)
        {
            return null;
        }

        offset += (int)vendorLength;
        if (!TryReadU32Le(payload, ref offset, out var commentCount))
        {
            return null;
        }

        var metadata = new MutableMetadata();
        for (var i = 0; i < commentCount; i++)
        {
            if (!TryReadU32Le(payload, ref offset, out var length) || offset + length > payload.Length)
            {
                break;
            }

            var comment = Encoding.UTF8.GetString(payload.Slice(offset, (int)length).ToArray());
            offset += (int)length;
            var split = comment.IndexOf('=');
            if (split <= 0)
            {
                continue;
            }

            var key = comment[..split].ToLowerInvariant();
            var value = Sanitize(comment[(split + 1)..]);
            if (string.IsNullOrWhiteSpace(value))
            {
                continue;
            }

            switch (key)
            {
                case "title":
                    metadata.Title ??= value;
                    break;
                case "artist" or "albumartist" or "album artist":
                    metadata.Artist ??= value;
                    break;
                case "album":
                    metadata.Album ??= value;
                    break;
                case "date" or "year" or "originaldate":
                    metadata.Year ??= value;
                    break;
                case "genre":
                    metadata.Genre ??= value;
                    break;
            }
        }

        return metadata.ToMetadata();
    }

    private static string? DecodeId3Text(ReadOnlySpan<byte> frame)
    {
        if (frame.Length <= 1)
        {
            return null;
        }

        var encoding = frame[0];
        var payload = frame[1..];
        string decoded;
        if (encoding is 0 or 3)
        {
            decoded = encoding == 3
                ? Encoding.UTF8.GetString(payload)
                : Encoding.Latin1.GetString(payload);
        }
        else if (encoding is 1 or 2)
        {
            var littleEndian = encoding == 1;
            var start = 0;
            if (encoding == 1 && payload.Length >= 2)
            {
                if (payload[0] == 0xFF && payload[1] == 0xFE)
                {
                    littleEndian = true;
                    start = 2;
                }
                else if (payload[0] == 0xFE && payload[1] == 0xFF)
                {
                    littleEndian = false;
                    start = 2;
                }
            }

            decoded = littleEndian
                ? Encoding.Unicode.GetString(payload[start..])
                : Encoding.BigEndianUnicode.GetString(payload[start..]);
        }
        else
        {
            return null;
        }

        var nullIndex = decoded.IndexOf('\0');
        if (nullIndex >= 0)
        {
            decoded = decoded[..nullIndex];
        }

        return Sanitize(decoded);
    }

    private static byte[] RemoveUnsynchronization(byte[] input)
    {
        var output = new List<byte>(input.Length);
        for (var i = 0; i < input.Length; i++)
        {
            output.Add(input[i]);
            if (input[i] == 0xFF && i + 1 < input.Length && input[i + 1] == 0)
            {
                i++;
            }
        }

        return [.. output];
    }

    private static uint ReadSynchsafe(byte[] bytes, int offset) =>
        (uint)((bytes[offset] & 0x7F) << 21 |
               (bytes[offset + 1] & 0x7F) << 14 |
               (bytes[offset + 2] & 0x7F) << 7 |
               (bytes[offset + 3] & 0x7F));

    private static uint ReadU32Be(byte[] bytes, int offset) =>
        ((uint)bytes[offset] << 24) |
        ((uint)bytes[offset + 1] << 16) |
        ((uint)bytes[offset + 2] << 8) |
        bytes[offset + 3];

    private static bool TryReadU32Le(ReadOnlySpan<byte> bytes, ref int offset, out uint value)
    {
        if (offset + 4 > bytes.Length)
        {
            value = 0;
            return false;
        }

        value = (uint)(bytes[offset] | (bytes[offset + 1] << 8) | (bytes[offset + 2] << 16) | (bytes[offset + 3] << 24));
        offset += 4;
        return true;
    }

    private static string? TrimLatin1(ReadOnlySpan<byte> bytes)
    {
        return Sanitize(Encoding.Latin1.GetString(bytes));
    }

    private static string? Sanitize(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return null;
        }

        var builder = new StringBuilder(value.Length);
        foreach (var ch in value)
        {
            if (ch == '\0')
            {
                continue;
            }

            builder.Append(char.IsControl(ch) ? ' ' : ch);
        }

        var cleaned = builder.ToString().Trim();
        return string.IsNullOrWhiteSpace(cleaned) ? null : cleaned;
    }

    private sealed class MutableMetadata
    {
        public string? Year { get; set; }
        public string? Artist { get; set; }
        public string? Album { get; set; }
        public string? Title { get; set; }
        public string? Genre { get; set; }
        public int? DurationSeconds { get; set; }
        public string? Source { get; set; }

        public MediaMetadata? ToMetadata()
        {
            if (Year is null && Artist is null && Album is null && Title is null && Genre is null && DurationSeconds is null)
            {
                return null;
            }

            return new MediaMetadata
            {
                Year = NormalizeYear(Year),
                Artist = Artist,
                Album = Album,
                Title = Title,
                Genre = Genre,
                DurationSeconds = DurationSeconds,
                Source = Source
            };
        }

        private static string? NormalizeYear(string? value)
        {
            if (string.IsNullOrWhiteSpace(value))
            {
                return null;
            }

            for (var i = 0; i <= value.Length - 4; i++)
            {
                if (char.IsDigit(value[i]) &&
                    char.IsDigit(value[i + 1]) &&
                    char.IsDigit(value[i + 2]) &&
                    char.IsDigit(value[i + 3]))
                {
                    return value.Substring(i, 4);
                }
            }

            return value.Trim();
        }
    }
}
