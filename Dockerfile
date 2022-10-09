FROM python:3.9
RUN apt-get update && apt-get -y install git ffmpeg libsm6 libxext6
RUN cd /home && mkdir /home/region && git clone https://github.com/louis-e/arnis.git
WORKDIR /home/arnis
RUN pip install -r requirements.txt
ENTRYPOINT ["python", "arnis.py"]