### bash_builtin_stdin_pipe_to_script
# echo | bash script.sh should forward stdin
echo 'echo "got: $(cat)"' > /tmp/stdin_test.sh
echo "hello" | bash /tmp/stdin_test.sh
### expect
got: hello
### end

### bash_builtin_stdin_pipe_to_c
# echo | bash -c 'cat' should forward stdin
echo "piped" | bash -c 'cat'
### expect
piped
### end

### bash_builtin_stdin_read
# echo | bash script.sh with read builtin
echo 'read -r line; echo "line: $line"' > /tmp/read_test.sh
echo "test input" | bash /tmp/read_test.sh
### expect
line: test input
### end

### empty_pipe_stays_connected_in_compound_commands
# An empty or exhausted pipe inherited by a group, subshell or loop body is
# connected input at EOF: wc reads it and prints 0 instead of nothing
: | { wc -c; }
: | ( wc -c )
: | while true; do wc -c; break; done
: | for i in 1; do wc -c; done
printf 'a\n' | { read x; wc -c; }
### expect
0
0
0
0
0
### end
