module Demo
  class User
    attr_reader :name

    def initialize(name)
      @name = name
    end

    def label(prefix = "")
      "#{prefix}#{name}"
    end
  end

  def self.normalize(value)
    value.strip
  end
end
