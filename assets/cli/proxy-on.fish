function proxy-on
    set -l proxy_url http://127.0.0.1:7890

    if not command curl -fsSI \
                        --proxy $proxy_url \
                        --connect-timeout 3 \
                        --max-time 8 \
                        https://github.com >/dev/null
        echo "proxy tunnel unavailable: $proxy_url" >&2
        return 1
    end

    set -gx http_proxy $proxy_url
    set -gx https_proxy $proxy_url
    set -gx all_proxy $proxy_url
    set -gx HTTP_PROXY $proxy_url
    set -gx HTTPS_PROXY $proxy_url
    set -gx ALL_PROXY $proxy_url

    set -gx no_proxy 127.0.0.1,localhost,::1
    set -gx NO_PROXY $no_proxy

    echo "proxy ON: $proxy_url"
end
