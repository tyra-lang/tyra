# 06-cli-args.rb
# Command-line argument parsing.
# Ruby: ARGV is a global array. No safe access — direct indexing returns nil on OOB.

def run_serve(args)
  port_str = args[1]
  abort "error: missing arg: port" unless port_str

  port = Integer(port_str)
  puts "serving on port #{port}"
rescue ArgumentError
  abort "error: invalid arg port: #{port_str}"
end

def run_help
  puts "usage: myapp <command> [args]"
  puts "commands:"
  puts "  serve <port>  start the server"
  puts "  help          show this message"
end

command = ARGV[0]
abort "error: missing command" unless command

case command
when "serve"
  run_serve(ARGV)
when "help"
  run_help
else
  abort "error: unknown command: #{command}"
end
