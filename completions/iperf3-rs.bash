# bash completion for iperf3-rs.

_iperf3_rs()
{
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    local opts="
        -s --server
        -c --client
        -p --port
        -f --format
        -i --interval
        -I --pidfile
        -F --file
        -B --bind
        --bind-dev
        -V --verbose
        -J --json
        --json-stream
        --json-stream-full-output
        --logfile
        --forceflush
        --timestamps
        --rcv-timeout
        -d --debug
        -v --version
        -h --help
        -D --daemon
        -1 --one-off
        --server-bitrate-limit
        --idle-timeout
        --server-max-duration
        --rsa-private-key-path
        --authorized-users-path
        --time-skew-threshold
        --use-pkcs1-padding
        -u --udp
        --connect-timeout
        -b --bitrate
        --pacing-timer
        -t --time
        -n --bytes
        -k --blockcount
        -l --length
        --cport
        -P --parallel
        -R --reverse
        --bidir
        -w --window
        -M --set-mss
        -N --no-delay
        -4 --version4
        -6 --version6
        -S --tos
        --dscp
        -Z --zerocopy
        --skip-rx-copy
        -O --omit
        -T --title
        --extra-data
        --get-server-output
        --udp-counters-64bit
        --gsro
        --repeating-payload
        --dont-fragment
        --username
        --rsa-public-key-path
        --push.url
        --push.job
        --push.label
        --push.timeout
        --push.retries
        --push.user-agent
        --push.metric-prefix
        --push.interval
    "

    case "${cur}" in
        --push.timeout=*)
            COMPREPLY=($(compgen -P "--push.timeout=" -W "500ms 1s 5s 10s 30s 1m" -- "${cur#*=}"))
            return
            ;;
        --push.retries=*)
            COMPREPLY=($(compgen -P "--push.retries=" -W "0 1 2 3 5 10" -- "${cur#*=}"))
            return
            ;;
        --push.metric-prefix=*)
            COMPREPLY=($(compgen -P "--push.metric-prefix=" -W "iperf3" -- "${cur#*=}"))
            return
            ;;
        --push.interval=*)
            COMPREPLY=($(compgen -P "--push.interval=" -W "500ms 1s 5s 10s 30s 1m" -- "${cur#*=}"))
            return
            ;;
        --push.label=*)
            COMPREPLY=($(compgen -P "--push.label=" -W "test= scenario= site= host=" -- "${cur#*=}"))
            return
            ;;
        --push.url=*)
            COMPREPLY=($(compgen -P "--push.url=" -W "http://127.0.0.1:9091 http://localhost:9091" -- "${cur#*=}"))
            return
            ;;
    esac

    case "${prev}" in
        -c|--client|-B|--bind)
            if declare -F _known_hosts_real >/dev/null 2>&1; then
                _known_hosts_real "${cur}"
            fi
            return
            ;;
        -I|--pidfile|-F|--file|--logfile|--rsa-private-key-path|--authorized-users-path)
            compopt -o default 2>/dev/null || true
            return
            ;;
        -f|--format)
            COMPREPLY=($(compgen -W "k m g t K M G T" -- "${cur}"))
            return
            ;;
        --push.timeout)
            COMPREPLY=($(compgen -W "500ms 1s 5s 10s 30s 1m" -- "${cur}"))
            return
            ;;
        --push.retries)
            COMPREPLY=($(compgen -W "0 1 2 3 5 10" -- "${cur}"))
            return
            ;;
        --push.metric-prefix)
            COMPREPLY=($(compgen -W "iperf3" -- "${cur}"))
            return
            ;;
        --push.interval)
            COMPREPLY=($(compgen -W "500ms 1s 5s 10s 30s 1m" -- "${cur}"))
            return
            ;;
        --push.label)
            COMPREPLY=($(compgen -W "test= scenario= site= host=" -- "${cur}"))
            return
            ;;
        --push.url)
            COMPREPLY=($(compgen -W "http://127.0.0.1:9091 http://localhost:9091" -- "${cur}"))
            return
            ;;
        --push.job)
            COMPREPLY=($(compgen -W "iperf3" -- "${cur}"))
            return
            ;;
        -p|--port|--rcv-timeout|--server-bitrate-limit|--idle-timeout|--server-max-duration|--time-skew-threshold|--connect-timeout|-b|--bitrate|--pacing-timer|-t|--time|-n|--bytes|-k|--blockcount|-l|--length|--cport|-P|--parallel|-w|--window|-M|--set-mss|-S|--tos|--dscp|-O|--omit|-T|--title|--extra-data|--username|--bind-dev|--push.user-agent)
            return
            ;;
    esac

    if [[ "${cur}" == -* ]]; then
        COMPREPLY=($(compgen -W "${opts}" -- "${cur}"))
    fi
}

complete -F _iperf3_rs iperf3-rs
