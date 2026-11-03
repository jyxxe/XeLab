"""Explicit OCI/XFS acceptance only: quota inheritance, persistence and access."""
import ctypes
import errno
import fcntl
import os
import struct
import sys

mode = sys.argv[1]
fd = os.open('/data', os.O_RDONLY | os.O_DIRECTORY)
attributes = fcntl.ioctl(fd, 0x801c581f, bytes(28))
flags, _, _, project, _ = struct.unpack('=5I8x', attributes)
assert project != 0 and flags & 0x200
libc = ctypes.CDLL(None, use_errno=True)
libc.ioctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_void_p]
changed = ctypes.create_string_buffer(struct.pack('=5I8x', flags & ~0x200, 0, 0, project, 0))
for command in [0x401c5820, 0x40086602, 0x40046602]:
    for high_bits in [0, 1 << 32]:
        ctypes.set_errno(0)
        assert libc.ioctl(fd, command | high_bits, changed) == -1
        assert ctypes.get_errno() == errno.EPERM, (command, ctypes.get_errno())
os.close(fd)
if mode == 'fill':
    with open('/data/sentinel', 'wb') as file:
        file.write(b'volume survives a container restart\n')
        file.flush()
        os.fsync(file.fileno())
    os.makedirs('/data/nested', exist_ok=True)
    child = os.open('/data/nested', os.O_RDONLY | os.O_DIRECTORY)
    child_attributes = fcntl.ioctl(child, 0x801c581f, bytes(28))
    assert struct.unpack('=5I8x', child_attributes)[3] == project
    os.close(child)
    total = 0
    try:
        with open('/data/nested/full', 'wb', buffering=0) as file:
            while total < 32 * 1024 * 1024:
                file.write(bytes(65536))
                os.fsync(file.fileno())
                total += 65536
    except OSError as error:
        assert error.errno in (errno.EDQUOT, errno.ENOSPC)
    else:
        raise RuntimeError('real quota was bypassed')
    assert total > 0
elif mode in ('read', 'ro'):
    with open('/data/sentinel', 'rb') as file:
        assert file.read() == b'volume survives a container restart\n'
    if mode == 'ro':
        try:
            with open('/data/must-not-write', 'wb'):
                pass
        except OSError as error:
            assert error.errno == errno.EROFS
        else:
            raise RuntimeError('read-only volume became writable')
else:
    raise RuntimeError('unknown volume acceptance mode')
print('volume-' + mode + '-verified')
