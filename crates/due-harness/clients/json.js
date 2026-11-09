// Shared strict JSON encoding for the embedded JavaScript runners.
globalThis.__dueJson = (function () {
  'use strict';
  var MAX_DEPTH = 64;
  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function isPlain(value) {
    var prototype = Object.getPrototypeOf(value);
    return prototype === Object.prototype || prototype === null;
  }

  function utf8Length(s) {
    var n = 0;
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c < 0x80) n += 1;
      else if (c < 0x800) n += 2;
      else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length &&
               s.charCodeAt(i + 1) >= 0xdc00 && s.charCodeAt(i + 1) <= 0xdfff) { n += 4; i++; }
      else n += 3;
    }
    return n;
  }

  // Strict JSON encoder for outbound values. Throws a plain Error naming the offending path.
  function encode(value, path, stack) {
    var t = typeof value;
    if (value === null) return 'null';
    if (t === 'boolean') return value ? 'true' : 'false';
    if (t === 'string') return JSON.stringify(value);
    if (t === 'number') {
      if (!isFinite(value)) throw new Error(path + ': non-finite number');
      if (Math.floor(value) === value && Math.abs(value) > 9007199254740991) throw new Error(path + ': unsafe integer');
      if (value === 0 && 1 / value < 0) throw new Error(path + ': negative zero');
      return JSON.stringify(value);
    }
    if (t !== 'object') throw new Error(path + ': unsupported ' + t);
    if (stack.length >= MAX_DEPTH) throw new Error(path + ': nesting too deep');
    if (stack.indexOf(value) >= 0) throw new Error(path + ': cycle');
    stack.push(value);
    var out, i;
    if (Array.isArray(value)) {
      var parts = [];
      if (Object.getOwnPropertySymbols(value).length > 0) throw new Error(path + ': symbol keys');
      if (Object.getOwnPropertyNames(value).length !== value.length + 1) throw new Error(path + ': sparse array or extra array properties');
      for (i = 0; i < value.length; i++) {
        var ad = Object.getOwnPropertyDescriptor(value, i);
        if (!ad) throw new Error(path + '[' + i + ']: sparse array');
        if (!has(ad, 'value')) throw new Error(path + '[' + i + ']: accessor property');
        parts.push(encode(value[i], path + '[' + i + ']', stack));
      }
      out = '[' + parts.join(',') + ']';
    } else {
      if (!isPlain(value)) throw new Error(path + ': not a plain object');
      if (Object.getOwnPropertySymbols(value).length > 0) throw new Error(path + ': symbol keys');
      var keys = Object.keys(value), members = [];
      if (Object.getOwnPropertyNames(value).length !== keys.length) throw new Error(path + ': non-enumerable properties');
      for (i = 0; i < keys.length; i++) {
        if (!has(Object.getOwnPropertyDescriptor(value, keys[i]), 'value')) throw new Error(path + '.' + keys[i] + ': accessor property');
        members.push(JSON.stringify(keys[i]) + ':' + encode(value[keys[i]], path + '.' + keys[i], stack));
      }
      out = '{' + members.join(',') + '}';
    }
    stack.pop();
    return out;
  }

  return Object.freeze({
    utf8Length: utf8Length,
    encode: function (value, path) { return encode(value, path, []); },
    result: function (value) {
      var encoded = encode(value === undefined ? null : value, 'result', []);
      if (utf8Length(encoded) > 1024 * 1024) throw new Error('workflow result exceeds 1 MiB');
      return encoded;
    },
  });
})();
