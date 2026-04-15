// 09-error-handling.v
// Error handling patterns.
// V uses !T for results with errors, `or` blocks, and `defer`.

import os

fn read_config(path string) !string {
	content := os.read_file(path) or {
		return error('file not found: ${path}')
	}
	return content
}

fn parse_port(config string) !int {
	port := config.int() or {
		return error('port must be an integer')
	}
	if port < 1 || port > 65535 {
		return error('invalid port: ${port}')
	}
	return port
}

fn start_server(port int) ! {
	if port == 0 {
		panic('port must not be zero')
	}
	println('starting server on port ${port}')
}

fn main() {
	config := read_config('app.conf') or {
		println('error: ${err.msg()}')
		return
	}
	port := parse_port(config) or {
		println('error: ${err.msg()}')
		return
	}
	start_server(port) or {
		println('error: ${err.msg()}')
		return
	}
}
