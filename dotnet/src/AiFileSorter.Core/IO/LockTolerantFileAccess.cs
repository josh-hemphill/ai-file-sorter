using System.Runtime.InteropServices;

namespace AiFileSorter.Core.IO;

/// <summary>Opens user files with share-read flags and never waits on exclusive locks.</summary>
public static class LockTolerantFileAccess
{
    public const FileShare SharedRead = FileShare.ReadWrite | FileShare.Delete;
    public static readonly TimeSpan DefaultLockTimeout = TimeSpan.FromMilliseconds(150);

    public static FileStream? TryOpenRead(string path, out string? lockReason)
    {
        return TryOpenRead(path, DefaultLockTimeout, out lockReason);
    }

    public static FileStream? TryOpenRead(string path, TimeSpan timeout, out string? lockReason)
    {
        lockReason = null;
        var deadline = DateTime.UtcNow + timeout;
        IOException? lastLock = null;

        while (true)
        {
            try
            {
                return new FileStream(
                    path,
                    FileMode.Open,
                    FileAccess.Read,
                    SharedRead,
                    bufferSize: 4096,
                    FileOptions.SequentialScan);
            }
            catch (FileNotFoundException)
            {
                lockReason = "File was removed while opening.";
                return null;
            }
            catch (DirectoryNotFoundException)
            {
                lockReason = "Parent directory is missing.";
                return null;
            }
            catch (UnauthorizedAccessException ex)
            {
                lockReason = "Access denied: " + ex.Message;
                return null;
            }
            catch (IOException ex) when (IsLockConflict(ex))
            {
                lastLock = ex;
                if (DateTime.UtcNow >= deadline)
                {
                    lockReason = "Locked by another process: " + ex.Message;
                    return null;
                }

                Thread.Sleep(20);
            }
        }
    }

    public static byte[]? TryReadPrefix(string path, int maxBytes, out string? lockReason)
    {
        using var stream = TryOpenRead(path, out lockReason);
        if (stream is null)
        {
            return null;
        }

        var buffer = new byte[Math.Max(0, maxBytes)];
        var read = stream.Read(buffer, 0, buffer.Length);
        if (read == buffer.Length)
        {
            return buffer;
        }

        var copy = new byte[read];
        Buffer.BlockCopy(buffer, 0, copy, 0, read);
        return copy;
    }

    public static byte[]? TryReadAll(string path, int maxBytes, out string? lockReason)
    {
        using var stream = TryOpenRead(path, out lockReason);
        if (stream is null)
        {
            return null;
        }

        if (stream.Length > maxBytes)
        {
            lockReason = $"File exceeds {maxBytes} byte read cap.";
            return null;
        }

        using var memory = new MemoryStream((int)stream.Length);
        stream.CopyTo(memory);
        return memory.ToArray();
    }

    public static bool IsLockConflict(IOException exception)
    {
        var code = exception.HResult & 0xFFFF;
        if (code is 32 or 33)
        {
            return true;
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return false;
        }

        var message = exception.Message;
        return message.Contains("lock", StringComparison.OrdinalIgnoreCase) ||
               message.Contains("busy", StringComparison.OrdinalIgnoreCase) ||
               message.Contains("text file busy", StringComparison.OrdinalIgnoreCase);
    }
}
