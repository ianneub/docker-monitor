FROM ruby:2.6

# throw errors if Gemfile has been modified since Gemfile.lock
RUN bundle config --global frozen 1

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock ./
RUN gem install bundler:2.1.4 && bundle install

COPY . .

CMD ["bundle", "exec", "ruby", "run.rb"]
