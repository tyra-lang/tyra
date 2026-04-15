// 06-cli-args.v
// Command-line argument parsing.
// V has os.args built in.

import os

fn run_serve(args []string) ! {
	if args.len < 3 {
		return error('missing arg: port')
	}
	port := args[2].int() or { return error('invalid arg port: ${args[2]}') }
	println('serving on port ${port}')
}

fn run_help() {
	println('usage: myapp <command> [args]')
	println('commands:')
	println('  serve <port>  start the server')
	println('  help          show this message')
}

fn main() {
	args := os.args
	if args.len < 2 {
		println('error: missing command')
		return
	}
	command := args[1]
	match command {
		'serve' {
			run_serve(args) or {
				println('error: ${err.msg()}')
			}
		}
		'help' { run_help() }
		else { println('error: unknown command: ${command}') }
	}
}
