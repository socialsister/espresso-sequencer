FROM ubuntu:jammy

RUN apt-get update \
    &&  rm -rf /var/lib/apt/lists/*

#COPY target2/x86_64_-unknown-linux-musl/deploy-example-contracts /bin/deploy-example-contracts
COPY target-2 /bin/deploy-example-contracts
RUN chmod +x /bin/deploy-example-contracts

CMD [ "/bin/deploy-example-contracts"]
