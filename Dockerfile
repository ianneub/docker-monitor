FROM ruby:3.4

ENV AWS_DEFAULT_REGION=us-east-1

# throw errors if Gemfile has been modified since Gemfile.lock
RUN bundle config --global frozen 1

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock ./
RUN gem install bundler:2.6.7 && bundle install

COPY . .

CMD ["bundle", "exec", "ruby", "run.rb"]
