# 06-cli-args.cr
# Command-line argument parsing.
# Crystal: ARGV is Array(String). Index returns T (raises on OOB).
# ARGV[n]? returns T? (nil on OOB).

def run_serve(args : Array(String))
  port_str = args[2]? || abort "error: missing arg: port"
  port = port_str.to_i? || abort "error: invalid arg port: #{port_str}"
  puts "serving on port #{port}"
end

def run_help
  puts "usage: myapp <command> [args]"
  puts "commands:"
  puts "  serve <port>  start the server"
  puts "  help          show this message"
end

command = ARGV[0]? || abort "error: missing command"

case command
when "serve"
  run_serve(ARGV)
when "help"
  run_help
else
  abort "error: unknown command: #{command}"
end
