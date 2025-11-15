#!/bin/bash

# 生成自签名SSL证书脚本
# 适用于本地开发环境

echo "正在生成本地SSL证书..."

# 创建临时配置文件
cat > openssl_temp.cnf << 'EOF'
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_req
prompt = no

[req_distinguished_name]
CN = localhost

[v3_req]
keyUsage = keyEncipherment, dataEncipherment, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = localhost
DNS.2 = 127.0.0.1
DNS.3 = ::1
EOF

# 生成证书和私钥
openssl req -x509 -newkey rsa:2048 -keyout localhost.key -out localhost.crt \
  -days 365 -nodes -config openssl_temp.cnf -extensions v3_req

# 检查是否成功
if [ $? -eq 0 ]; then
    echo "✓ 证书生成成功！"
    echo "✓ 私钥文件: localhost.key"
    echo "✓ 证书文件: localhost.crt"
    
    # 显示证书信息
    echo ""
    echo "证书信息:"
    openssl x509 -in localhost.crt -text -noout | grep -E "Subject:|Not Before|Not After|DNS:"
else
    echo "✗ 证书生成失败！"
fi

# 清理临时文件
rm -f openssl_temp.cnf

echo ""
echo "使用方法:"
echo "1. 将 localhost.crt 添加到系统信任证书库"
echo "2. 在web服务器配置中使用 localhost.key"

