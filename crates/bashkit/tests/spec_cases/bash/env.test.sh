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
### bash_diff: GNU env reports "env: 'nosuch': No such file or directory"
env nosuch-command 2>/dev/null; echo "rc=$?"
### expect
rc=127
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
