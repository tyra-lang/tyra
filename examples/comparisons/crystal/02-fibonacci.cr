# 02-fibonacci.cr
# Recursive Fibonacci.

def fib(n : Int32) : Int32
  case n
  when 0 then 0
  when 1 then 1
  else        fib(n - 1) + fib(n - 2)
  end
end

result = fib(10)
puts "fib(10) = #{result}"
