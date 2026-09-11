"""Bounded QMP commands for firmware-test device attachment."""
import json
import socket


def change_serial(control, device, backend):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(5)
        client.connect(str(control))
        with client.makefile('rwb') as stream:
            def receive():
                line = stream.readline(65537)
                if not line or len(line) > 65536:
                    raise RuntimeError('invalid QMP response')
                return json.loads(line)

            if 'QMP' not in receive():
                raise RuntimeError('missing QMP greeting')

            def execute(command, arguments):
                stream.write(json.dumps({'execute': command, 'arguments': arguments,
                                         'id': command}).encode() + b'\n')
                stream.flush()
                for _ in range(32):
                    response = receive()
                    if response.get('id') == command:
                        if 'return' not in response:
                            raise RuntimeError(f'QMP {command} failed: {response}')
                        return
                raise RuntimeError('QMP event limit exceeded')

            execute('qmp_capabilities', {})
            execute('chardev-change', {
                'id': device,
                'backend': backend,
            })


def attach_serial(control, device, endpoint):
    change_serial(control, device, {'type': 'socket', 'data': {
                    'addr': {'type': 'unix', 'data': {'path': str(endpoint)}},
                    'server': False,
                }})


def detach_serial(control, device):
    change_serial(control, device, {'type': 'null', 'data': {}})
