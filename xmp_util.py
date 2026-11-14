import os
import struct

def extract_app_markers(jpeg_bytes):
    """Extract APP1 (Exif & XMP) and other metadata markers from a JPEG."""
    markers = []
    if not jpeg_bytes.startswith(b'\xff\xd8'):
        return markers
    
    idx = 2
    length = len(jpeg_bytes)
    while idx < length - 4:
        if jpeg_bytes[idx] != 0xff:
            break
        marker = jpeg_bytes[idx:idx+2]
        if marker in (b'\xff\xd9', b'\xff\xda'): # EOI or SOS
            break
        
        # Read segment length
        seg_len = struct.unpack('>H', jpeg_bytes[idx+2:idx+4])[0]
        seg_data = jpeg_bytes[idx:idx+2+seg_len]
        
        # We care about APP1 (0xFFE1) which carries EXIF and XMP
        # and APP2 (ICC), APP0
        if marker in (b'\xff\xe1', b'\xff\xe2'):
            markers.append(seg_data)
        
        idx += 2 + seg_len
    return markers

def inject_markers_to_jpeg(target_jpeg_bytes, markers):
    """Inject metadata markers into a new JPEG right after SOI (0xFFD8)."""
    if not target_jpeg_bytes.startswith(b'\xff\xd8'):
        return target_jpeg_bytes
    
    # Check if target already has APP1 markers and strip them or avoid duplicating
    idx = 2
    while idx < len(target_jpeg_bytes) - 4:
        if target_jpeg_bytes[idx] != 0xff:
            break
        marker = target_jpeg_bytes[idx:idx+2]
        if marker == b'\xff\xda': # SOS
            break
        seg_len = struct.unpack('>H', target_jpeg_bytes[idx+2:idx+4])[0]
        if marker == b'\xff\xe1':
            # Skip existing basic APP1 if we are replacing with rich source markers
            pass
        idx += 2 + seg_len
        
    return b'\xff\xd8' + b''.join(markers) + target_jpeg_bytes[2:]

print("XMP extractor & injector defined.")
