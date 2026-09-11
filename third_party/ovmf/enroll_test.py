"""Exercise malformed templates and the on-flash UEFI authenticated layout."""
import struct
import unittest

from enroll import AUTH_STORE, GLOBAL, enroll, signature_list, variable


def template():
    result = bytearray(b'\xff' * 8192)
    result[40:44] = b'_FVH'
    struct.pack_into('<H', result, 48, 72)
    result[72:88] = AUTH_STORE.bytes_le
    struct.pack_into('<I', result, 88, len(result) - 72)
    result[92:94] = b'\x5a\xfe'
    return result


class EnrollmentTests(unittest.TestCase):
    def test_x86_data_immediately_follows_utf16_name(self):
        data = b'certificate marker'
        record = variable('PK', GLOBAL, 0x27, data)
        self.assertEqual(record[60:66], b'P\0K\0\0\0')
        self.assertEqual(record[66:66+len(data)], data)
        self.assertEqual(len(record) % 4, 0)

    def test_walk_complete_store_and_signature_database(self):
        certificate = b'certificate fixture'
        output = enroll(template(), certificate, revoke=True)
        cursor = 100
        names = []
        while output[cursor:cursor+2] == b'\xaa\x55':
            name_size, data_size = struct.unpack_from('<II', output, cursor+36)
            name_start = cursor + 60
            data_start = name_start + name_size
            name = output[name_start:data_start].decode('utf-16-le').rstrip('\0')
            names.append(name)
            if name in ('PK', 'KEK', 'db', 'dbx'):
                self.assertEqual(output[data_start:data_start+data_size], signature_list(certificate))
            cursor = (data_start + data_size + 3) & ~3
        self.assertEqual(names, ['PK', 'KEK', 'db', 'dbx', 'SecureBootEnable', 'CustomMode'])
        self.assertTrue(all(byte == 0xff for byte in output[cursor:]))

    def test_existing_enrollment_cannot_be_overwritten(self):
        with self.assertRaisesRegex(ValueError, 'existing variable store'):
            enroll(enroll(template(), b'cert'), b'other cert')

    def test_invalid_and_truncated_stores_are_rejected(self):
        for offset, value in [(40, b'BAD!'), (48, b'\xff\xff'), (72, b'bad'),
                              (88, b'\xff\xff\xff\xff'), (92, b'\0\0')]:
            broken = template()
            broken[offset:offset+len(value)] = value
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                enroll(broken, b'cert')
        with self.assertRaises(ValueError):
            enroll(template()[:100], b'cert')


if __name__ == '__main__':
    unittest.main()
