#
# WilOS-Core default .bashrc. Users rarely see this because tty1 is
# wilai-shell; this fires on tty2–tty6 or over ssh.
#
[[ $- != *i* ]] && return

PS1='\[\e[36m\]wilos\[\e[0m\]:\w\$ '
alias ls='ls --color=auto'
alias ll='ls -lah'
alias grep='grep --color=auto'

# Convenience: `wi` opens Wilai from an interactive shell.
alias wi='wilai-shell --socket /run/wilai.sock'

cat <<'MSG'
This machine runs WilOS-Core. The primary interface is Wilai on tty1.
Type `wi` here to open a chat with Wilai from this shell.
MSG
