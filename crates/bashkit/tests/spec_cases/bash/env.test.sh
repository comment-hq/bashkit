### env_no_args_empty
### bash_diff: VFS env starts empty
# env with no args on empty environment
env | wc -l
### expect
0
### end

### env_ignore_environment
### bash_diff: VFS env starts empty so -i is same as default
# env -i starts with empty environment
env -i | wc -l
### expect
0
### end

### env_set_vars
### bash_diff: VFS env starts empty
# env with NAME=VALUE prints specified vars
env FOO=bar BAZ=qux | grep -c "="
### expect
2
### end

### env_runs_command
# env NAME=VALUE COMMAND runs COMMAND with the assignment in its environment
env FOO=1 printenv FOO
env printf '%s\n' hi
env -- echo dashdash
### expect
1
hi
dashdash
### end

### env_assignment_is_command_scoped
# env assignments do not leak into the calling shell
env FOO=1 true; echo "[${FOO-unset}]"
### expect
[unset]
### end

### env_command_reads_stdin_and_status
# env passes stdin to COMMAND and returns its exit status
printf 'a\nb\n' | env FOO=1 wc -l
env false; echo "rc=$?"
### expect
2
rc=1
### end

### env_missing_command
# env reports a missing COMMAND itself, with GNU quote() quoting
env nosuch-command 2>&1; echo "rc=$?"
LC_ALL=C env nosuch-command 2>&1
### expect
env: ‘nosuch-command’: No such file or directory
rc=127
env: 'nosuch-command': No such file or directory
### end

### env_skips_shell_functions
# env runs programs, not shell functions
greet() { echo function; }
env greet 2>&1; echo "rc=$?"
echo() { printf 'function echo\n'; }
env echo builtin
### expect
env: ‘greet’: No such file or directory
rc=127
builtin
### end

### env_i_hides_inherited_variables
# env -i runs COMMAND with only its own assignments
export FOO=bar
env -i printenv FOO; echo "rc=$?"
env -i A=1 printenv
env printenv FOO
echo "after=$FOO"
### expect
rc=1
A=1
bar
after=bar
### end
