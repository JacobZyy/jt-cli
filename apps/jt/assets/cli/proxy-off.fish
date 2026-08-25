function proxy-off
    set -eg http_proxy https_proxy all_proxy
    set -eg HTTP_PROXY HTTPS_PROXY ALL_PROXY
    set -eg no_proxy NO_PROXY

    echo "proxy OFF"
end
