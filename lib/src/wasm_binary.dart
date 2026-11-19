import 'dart:convert';
import 'dart:typed_data';

/// Minimal WASM binary reader used to extract custom sections
/// (wasm_run does not expose a custom-section API, so the host parses
/// the raw module bytes itself before compiling).
class WasmBinary {
  WasmBinary._();

  /// Returns the UTF-8 payload of the custom section named [name],
  /// or null when the module does not carry it.
  static String? customSection(Uint8List bytes, String name) {
    if (bytes.length < 8) return null;
    // Magic \0asm + version 1
    if (bytes[0] != 0x00 ||
        bytes[1] != 0x61 ||
        bytes[2] != 0x73 ||
        bytes[3] != 0x6d) {
      return null;
    }
    var offset = 8;
    while (offset < bytes.length) {
      final id = bytes[offset++];
      final size = _readLeb128(bytes, offset);
      offset = size.$2;
      final end = offset + size.$1;
      if (end > bytes.length) return null;
      if (id == 0x00) {
        // Custom section: name (leb-prefixed utf8) + payload.
        final nameLen = _readLeb128(bytes, offset);
        final nameEnd = nameLen.$2 + nameLen.$1;
        if (nameEnd > end) return null;
        final sectionName = utf8.decode(bytes.sublist(nameLen.$2, nameEnd));
        if (sectionName == name) {
          return utf8.decode(bytes.sublist(nameEnd, end));
        }
      }
      offset = end;
    }
    return null;
  }

  /// Reads an unsigned LEB128 value. Returns (value, nextOffset).
  static (int, int) _readLeb128(Uint8List bytes, int offset) {
    var result = 0;
    var shift = 0;
    var i = offset;
    while (i < bytes.length) {
      final byte = bytes[i++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) == 0) break;
      shift += 7;
    }
    return (result, i);
  }
}
